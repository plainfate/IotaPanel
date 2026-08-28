// SPDX-License-Identifier: Apache-2.0
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! MCP Agent 插件（纯 Rust 重写）。
//!
//! 轻量 MCP 服务器：AI 客户端经 HTTP POST /mcp（JSON-RPC 2.0、
//! Authorization: Bearer <token>）读取面板状态/插件/日志，并可控制插件进程。
//!
//! 配置位于 $PANEL_HOME/etc/mcp-agent/config.yaml：
//!   panel_addr / admin_user / admin_password / enable_read / enable_write / allow_shell

use iotapanel_sdk::http::{Request, Response};
use iotapanel_sdk::util::{self, Yaml};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

const INDEX_HTML: &str = include_str!("../web/index.html");

const DEFAULT_CONFIG: &str = "# mcp-agent 配置（写操作需要面板管理员凭据）\n\
panel_addr: 127.0.0.1:8787\n\
admin_user: admin\n\
admin_password: \"\"\n\
enable_read: true\n\
enable_write: true\n\
# 高危：设为 true 后开放 run_command（AI 可直接执行 shell 命令）\n\
allow_shell: false\n";

#[derive(Clone)]
struct Config {
    panel_addr: String,
    admin_user: String,
    admin_password: String,
    enable_read: bool,
    enable_write: bool,
    allow_shell: bool,
}

impl Config {
    fn default() -> Self {
        Config {
            panel_addr: "127.0.0.1:8787".into(),
            admin_user: "admin".into(),
            admin_password: String::new(),
            enable_read: true,
            enable_write: true,
            allow_shell: false,
        }
    }
    fn from_yaml(y: &Yaml) -> Self {
        Config {
            panel_addr: y.str_or("panel_addr", "127.0.0.1:8787"),
            admin_user: y.str_or("admin_user", "admin"),
            admin_password: y.str_or("admin_password", ""),
            enable_read: y.bool_or("enable_read", true),
            enable_write: y.bool_or("enable_write", true),
            allow_shell: y.bool_or("allow_shell", false),
        }
    }
    fn to_yaml(&self) -> String {
        format!(
            "# mcp-agent 配置（写操作需要面板管理员凭据）\n\
panel_addr: {}\n\
admin_user: {}\n\
admin_password: \"{}\"\n\
enable_read: {}\n\
enable_write: {}\n\
# 高危：设为 true 后开放 run_command（AI 可直接执行 shell 命令）\n\
allow_shell: {}\n",
            self.panel_addr, self.admin_user, self.admin_password, self.enable_read, self.enable_write, self.allow_shell
        )
    }
}

fn home_dir() -> PathBuf {
    std::env::var("PANEL_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/data/panel"))
}

fn cfg_dir() -> PathBuf {
    home_dir().join("etc").join("mcp-agent")
}

fn load_config(dir: &Path) -> Config {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("config.yaml");
    if !path.exists() {
        let _ = std::fs::write(&path, DEFAULT_CONFIG);
    }
    match std::fs::read_to_string(&path) {
        Ok(txt) => Config::from_yaml(&util::parse_yaml(&txt)),
        Err(_) => Config::default(),
    }
}

fn save_config(dir: &Path, cfg: &Config) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(dir);
    std::fs::write(dir.join("config.yaml"), cfg.to_yaml())
}

fn load_or_gen_token(dir: &Path) -> String {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("token");
    if let Ok(txt) = std::fs::read_to_string(&path) {
        let t = txt.trim();
        if !t.is_empty() && t.len() >= 16 {
            return t.to_string();
        }
    }
    let token = rand_hex(32);
    let _ = std::fs::write(&path, &token);
    token
}

/// 密码学安全随机十六进制串（直接读 /dev/urandom，无第三方依赖；
/// 用于 MCP Bearer 令牌，不能用非加密哈希凑数）。
fn rand_hex(len: usize) -> String {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").expect("open /dev/urandom");
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).expect("read /dev/urandom");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

// ---------- 面板 HTTP 客户端（纯 std）----------

struct PanelClient {
    addr: String,
    user: String,
    pass: String,
    cookie: String,
}

impl PanelClient {
    fn new(addr: &str, user: &str, pass: &str) -> Self {
        PanelClient { addr: addr.to_string(), user: user.to_string(), pass: pass.to_string(), cookie: String::new() }
    }

    fn raw_request(&mut self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        let mut req = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
            method, path, self.addr
        );
        if !self.cookie.is_empty() {
            req.push_str(&format!("Cookie: mp_session={}\r\n", self.cookie));
        }
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        req.push_str(body);
        match TcpStream::connect(self.addr.as_str()) {
            Ok(mut s) => {
                let _ = s.write_all(req.as_bytes());
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                parse_http_response(&buf)
            }
            Err(_) => (0, b"connection failed".to_vec()),
        }
    }

    fn api(&mut self, method: &str, path: &str, body: &str) -> (u16, Vec<u8>) {
        if self.cookie.is_empty() && !self.pass.is_empty() {
            self.try_login();
        }
        let (code, b) = self.raw_request(method, path, body);
        if code == 401 && !self.pass.is_empty() {
            self.try_login();
            return self.raw_request(method, path, body);
        }
        (code, b)
    }

    fn try_login(&mut self) {
        let payload = serde_json::json!({"username": self.user, "password": self.pass, "api": true}).to_string();
        let (code, buf) = self.raw_request("POST", "/api/login", &payload);
        if code == 200 {
            if let Ok(txt) = String::from_utf8(buf) {
                for part in txt.split("mp_session=") {
                    if part.len() > 2 {
                        let v = part.split(';').next().unwrap_or("").trim().to_string();
                        if !v.is_empty() {
                            self.cookie = v;
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn parse_http_response(buf: &[u8]) -> (u16, Vec<u8>) {
    let text = String::from_utf8_lossy(buf);
    let mut status = 0u16;
    let mut body = Vec::new();
    // 找头部结束
    if let Some(pos) = find_sub(buf, b"\r\n\r\n") {
        let head = &text[..pos];
        if let Some(line) = head.lines().next() {
            let mut it = line.split_whitespace();
            it.next();
            status = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        body = buf[pos + 4..].to_vec();
    }
    (status, body)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------- MCP 工具 ----------

fn tool_defs() -> Vec<serde_json::Value> {
    let obj = |props: serde_json::Map<String, serde_json::Value>| {
        serde_json::json!({"type": "object", "properties": props, "additionalProperties": false})
    };
    vec![
        serde_json::json!({"name": "get_status", "description": "面板总体状态：管理员、插件数、运行中插件、监听配置", "inputSchema": obj(serde_json::Map::new())}),
        serde_json::json!({"name": "list_plugins", "description": "列出已安装插件（名称/标题/版本/是否运行）", "inputSchema": obj(serde_json::Map::new())}),
        serde_json::json!({"name": "get_logs", "description": "读取面板核心日志末尾（lines 默认 80）", "inputSchema": obj(serde_json::json!({"lines": {"type": "number"}}).as_object().unwrap().clone())}),
        serde_json::json!({"name": "get_metrics", "description": "系统资源：CPU 负载、内存、磁盘、进程数", "inputSchema": obj(serde_json::Map::new())}),
        serde_json::json!({"name": "plugin_action", "description": "控制插件进程：action=start|stop|restart|keepalive，keepalive 需 enabled 布尔", "inputSchema": obj(serde_json::json!({"plugin": {"type": "string"}, "action": {"type": "string", "enum": ["start", "stop", "restart", "keepalive"]}, "enabled": {"type": "boolean"}}).as_object().unwrap().clone())}),
        serde_json::json!({"name": "run_command", "description": "执行 shell 命令（需 config.yaml 设 allow_shell: true；高危）", "inputSchema": obj(serde_json::json!({"command": {"type": "string"}}).as_object().unwrap().clone())}),
    ]
}

fn get_status_tool(home: &str, _args: &serde_json::Value) -> Result<String, String> {
    let panelpath = Path::new(home).join("data/panel.json");
    let users = std::fs::read_to_string(&panelpath).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v.get("users").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0)).unwrap_or(0);
    let plugins = std::fs::read_to_string(&panelpath).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v.get("plugins").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0)).unwrap_or(0);
    let running = std::fs::read_to_string(Path::new(home).join("etc/port-map.json")).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v.as_object().map(|o| o.len()).unwrap_or(0)).unwrap_or(0);
    let env = util::parse_env_file(&Path::new(home).join("etc/.env"));
    let listen = env.get("LISTEN_ADDR").cloned().unwrap_or_default();
    let trust = env.get("PANEL_TRUST_PROXY").cloned().unwrap_or_default();
    Ok(format!(
        "panel_home={}\nlisten_addr={}\nadmin_created={}\nplugins_installed={}\nplugins_running={}\ntrust_proxy={}",
        home, listen, users > 0, plugins, running, trust
    ))
}

fn list_plugins_tool(home: &str) -> Result<String, String> {
    let dir = Path::new(home).join("plugins");
    let ports = std::fs::read_to_string(Path::new(home).join("etc/port-map.json")).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for name in names {
            let mf_path = dir.join(&name).join("manifest.yaml");
            let (title, version) = std::fs::read_to_string(&mf_path).ok()
                .map(|s| {
                    let y = util::parse_yaml(&s);
                    (y.str_or("title", &name), y.str_or("version", "0"))
                })
                .unwrap_or_else(|| (name.clone(), "?".into()));
            let running = ports.as_object().map(|o| o.contains_key(&name)).unwrap_or(false);
            out.push(format!("- {} ({} v{}) running={}", name, title, version, running));
        }
    }
    Ok(out.join("\n"))
}

fn get_logs_tool(home: &str, args: &serde_json::Value) -> Result<String, String> {
    let lines = args.get("lines").and_then(|l| l.as_u64()).map(|n| n as usize).unwrap_or(80).max(1).min(2000);
    let data = std::fs::read_to_string(Path::new(home).join("logs/panel.log")).unwrap_or_default();
    let all: Vec<&str> = data.trim_end().lines().collect();
    let start = all.len().saturating_sub(lines);
    Ok(all[start..].join("\n"))
}

fn get_metrics_tool() -> Result<String, String> {
    run_shell("uptime; free -m | head -2; df -h / | tail -1; echo processes=$(ps -e --no-headers | wc -l)")
}

fn run_shell(cmd: &str) -> Result<String, String> {
    std::process::Command::new("sh").arg("-c").arg(cmd).output()
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            if !o.stderr.is_empty() {
                s.push_str(&String::from_utf8_lossy(&o.stderr));
            }
            s
        })
        .map_err(|e| e.to_string())
}

fn plugin_action_tool(cfg: &Config, pc: &mut PanelClient, args: &serde_json::Value) -> Result<String, String> {
    if !cfg.enable_write {
        return Err("写操作已关闭（可在配置页开启）".into());
    }
    let plugin = args.get("plugin").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if plugin.is_empty() {
        return Err("缺少 plugin".into());
    }
    let mut body = String::new();
    match action.as_str() {
        "start" | "stop" | "restart" => {}
        "keepalive" => {
            let enabled = args.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            body = serde_json::json!({"enabled": enabled}).to_string();
        }
        _ => return Err("action 需为 start/stop/restart/keepalive".into()),
    }
    let path = format!("/api/plugins/{}/{}{}", &plugin, &action, "");
    let (code, b) = pc.api("POST", &path, &body);
    if code == 0 {
        return Err("无法连接面板（请检查 config.yaml 的 panel_addr）".into());
    }
    if code != 200 {
        return Err(format!("面板返回 HTTP {}: {}", code, String::from_utf8_lossy(&b).trim()));
    }
    Ok(String::from_utf8_lossy(&b).trim().to_string())
}

fn call_tool(
    name: &str,
    args: &serde_json::Value,
    home: &str,
    cfg: &Config,
    pc: &mut PanelClient,
) -> Result<String, String> {
    match name {
        "get_status" => {
            if !cfg.enable_read { return Err("只读工具已关闭".into()); }
            get_status_tool(home, args)
        }
        "list_plugins" => {
            if !cfg.enable_read { return Err("只读工具已关闭".into()); }
            list_plugins_tool(home)
        }
        "get_logs" => {
            if !cfg.enable_read { return Err("只读工具已关闭".into()); }
            get_logs_tool(home, args)
        }
        "get_metrics" => {
            if !cfg.enable_read { return Err("只读工具已关闭".into()); }
            get_metrics_tool()
        }
        "plugin_action" => plugin_action_tool(cfg, pc, args),
        "run_command" => {
            if !cfg.allow_shell {
                return Err("run_command 未开启：请先在 config.yaml 设 allow_shell: true".into());
            }
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            run_shell(cmd)
        }
        _ => Err(format!("未知工具: {}", name)),
    }
}

// ---------- JSON-RPC ----------

struct RpcReq {
    method: String,
    id: Option<serde_json::Value>,
    params: serde_json::Value,
}

fn handle_rpc(body: &[u8], home: &str, cfg: &Config, pc: &mut PanelClient) -> Response {
    let req: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return Response::json_status(400, &serde_json::json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": "解析失败"}}));
        }
    };
    // 支持批量? 简化单对象
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let id = req.get("id").cloned();
    let params = req.get("params").cloned().unwrap_or(serde_json::json!({}));
    let has_id = id.is_some();

    let mut resp = serde_json::json!({"jsonrpc": "2.0"});

    let result: Result<serde_json::Value, serde_json::Value>;
    match method.as_str() {
        "initialize" => {
            result = Ok(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "iotapanel-mcp", "version": "0.2.0"}
            }));
        }
        "notifications/initialized" | "notifications/cancelled" | "shutdown" => result = Ok(serde_json::json!({})),
        "ping" => result = Ok(serde_json::json!({})),
        "tools/list" => result = Ok(serde_json::json!({"tools": tool_defs()})),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));
            match call_tool(&name, &args, home, cfg, pc) {
                Ok(text) => result = Ok(serde_json::json!({"content": [{"type": "text", "text": text}], "isError": false})),
                Err(e) => result = Err(serde_json::json!({"code": -32602, "message": e})),
            }
        }
        _ => result = Err(serde_json::json!({"code": -32601, "message": format!("未知方法: {}", method)})),
    }

    if has_id {
        match result {
            Ok(v) => { resp["result"] = v; }
            Err(e) => { resp["error"] = e; }
        }
        resp["id"] = id.unwrap_or(serde_json::Value::Null);
        Response::json(&resp)
    } else {
        // 无 id：notification，Go 版对无 id 的不回 body；回空 204
        Response::new(204)
    }
}

// ---------- HTTP handler ----------

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn check_bearer(req: &Request, token: &str) -> bool {
    req.header("authorization")
        .map(|h| {
            if let Some(rest) = h.strip_prefix("Bearer ") {
                constant_time_eq(rest.trim(), token)
            } else {
                false
            }
        })
        .unwrap_or(false)
}

fn main() {
    let dir = cfg_dir();
    let token = load_or_gen_token(&dir);
    let home = home_dir().to_string_lossy().into_owned();

    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(19006);

    // 页面 token 注入
    let page = INDEX_HTML.replace("__TOKEN__", &token);

    let handler = {
        let page = page.clone();
        let token = token.clone();
        let home = home.clone();
        move |req: &Request| {
            let cfg = load_config(&dir);
            let mut pc = PanelClient::new(&cfg.panel_addr, &cfg.admin_user, &cfg.admin_password);
            match req.path.as_str() {
                "/mcp" => {
                    if !check_bearer(req, &token) {
                        return Response::json_status(401, &serde_json::json!({"jsonrpc": "2.0", "error": {"code": -32001, "message": "无效的访问令牌"}}));
                    }
                    handle_rpc(&req.body, &home, &cfg, &mut pc)
                }
                "/api/config" if req.method == "GET" => {
                    if !check_bearer(req, &token) { return Response::json_err(401, "未授权"); }
                    Response::json(&serde_json::json!({
                        "panel_addr": cfg.panel_addr,
                        "admin_user": cfg.admin_user,
                        "enable_read": cfg.enable_read,
                        "enable_write": cfg.enable_write,
                        "allow_shell": cfg.allow_shell,
                        "admin_password": if cfg.admin_password.is_empty() { "" } else { "***" },
                    }))
                }
                "/api/config" if req.method == "POST" => {
                    if !check_bearer(req, &token) { return Response::json_err(401, "未授权"); }
                    let body: serde_json::Value = match serde_json::from_slice(&req.body) {
                        Ok(v) => v,
                        Err(_) => return Response::json_err(400, "非法 JSON"),
                    };
                    let mut nc = cfg.clone();
                    if let Some(a) = body.get("panel_addr").and_then(|v| v.as_str()) { nc.panel_addr = a.to_string(); }
                    if let Some(a) = body.get("admin_user").and_then(|v| v.as_str()) { nc.admin_user = a.to_string(); }
                    if let Some(a) = body.get("admin_password").and_then(|v| v.as_str()) { nc.admin_password = a.to_string(); }
                    if let Some(v) = body.get("enable_read").and_then(|v| v.as_bool()) { nc.enable_read = v; }
                    if let Some(v) = body.get("enable_write").and_then(|v| v.as_bool()) { nc.enable_write = v; }
                    if let Some(v) = body.get("allow_shell").and_then(|v| v.as_bool()) { nc.allow_shell = v; }
                    match save_config(&dir, &nc) {
                        Ok(_) => Response::json(&serde_json::json!({"ok": true})),
                        Err(e) => Response::json_err(500, &format!("保存失败: {}", e)),
                    }
                }
                _ => {
                    let mut r = Response::html(page.as_str());
                    r.headers.push(("Cache-Control".into(), "no-cache".into()));
                    r
                }
            }
        }
    };

    eprintln!("[mcp-agent] listening on {}:{} (token: {})", bind, port, token);
    if let Err(e) = iotapanel_sdk::http::serve(&bind, port, handler) {
        eprintln!("[mcp-agent] server error: {}", e);
        std::process::exit(1);
    }
}