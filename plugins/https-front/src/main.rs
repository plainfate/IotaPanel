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

//! HTTPS 网关插件（纯 Rust 重写）。
//!
//! 为面板提供 HTTPS 入口：监听 TLS 端口（默认 443），把解密后的 HTTP
//! 请求反向代理到面板监听地址。证书来源：
//!   - mode=selfsigned：首次运行自动用 rcgen 生成自签证书
//!   - mode=cert：读取已有 PEM 证书 + 私钥
//!   - mode=acme：本环境不支持自动签发（无 ACME 客户端库），回退到 cert/selfsigned，
//!     并在页面提示需自行使用 certbot 等签发。
//!
//! 双监听器设计：
//!   - 面板网关经 `/p/https-front/` 用**普通 HTTP** 访问本插件，故配置/状态服务
//!     必须绑定 `PLUGIN_PORT`（面板注入的本地端口），供网关反代。
//!   - 真实 HTTPS 前端（TLS 终结 + 反代回面板）绑定 config.yaml 里的 `https_port`。

use iotapanel_sdk::http::{Request, Response};
use iotapanel_sdk::util::{self, Yaml};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("../web/index.html");

#[derive(Clone)]
struct Cfg {
    panel_addr: String,
    mode: String,      // selfsigned | cert | acme
    cert_file: String, // mode=cert
    key_file: String,
    domain: String,     // selfsigned CN / acme 域名
    email: String,      // acme 联系邮箱
    https_port: u16,    // 对外 TLS 端口
    acme_http_addr: String, // acme HTTP-01 挑战监听（本环境未启用）
}

impl Cfg {
    fn default() -> Self {
        Cfg {
            panel_addr: "127.0.0.1:8787".into(),
            mode: "selfsigned".into(),
            cert_file: String::new(),
            key_file: String::new(),
            domain: "localhost".into(),
            email: String::new(),
            https_port: 443,
            acme_http_addr: ":80".into(),
        }
    }
    fn from_yaml(y: &Yaml) -> Self {
        Cfg {
            panel_addr: y.str_or("panel_addr", "127.0.0.1:8787"),
            mode: y.str_or("mode", "selfsigned"),
            cert_file: y.str_or("cert_file", ""),
            key_file: y.str_or("key_file", ""),
            domain: y.str_or("domain", "localhost"),
            email: y.str_or("email", ""),
            https_port: y.str_or("https_port", "443").parse().unwrap_or(443),
            acme_http_addr: y.str_or("acme_http_addr", ":80"),
        }
    }
    fn to_yaml(&self) -> String {
        format!(
            "panel_addr: {}\nmode: {}\ncert_file: \"{}\"\nkey_file: \"{}\"\ndomain: {}\nemail: \"{}\"\nhttps_port: {}\nacme_http_addr: {}\n",
            self.panel_addr,
            self.mode,
            self.cert_file,
            self.key_file,
            self.domain,
            self.email,
            self.https_port,
            self.acme_http_addr
        )
    }
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("PANEL_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/data/panel"))
}

fn cfg_path() -> std::path::PathBuf {
    let d = home_dir().join("etc").join("https-front");
    let _ = std::fs::create_dir_all(&d);
    d.join("config.yaml")
}

fn load_cfg() -> Cfg {
    let path = cfg_path();
    if !path.exists() {
        let _ = std::fs::write(&path, Cfg::default().to_yaml());
    }
    std::fs::read_to_string(&path)
        .map(|t| Cfg::from_yaml(&util::parse_yaml(&t)))
        .unwrap_or_else(|_| Cfg::default())
}

fn save_cfg(cfg: &Cfg) -> std::io::Result<()> {
    std::fs::write(cfg_path(), cfg.to_yaml())
}

/// 自签证书生成：返回 (cert_pem, key_pem)。rcgen 0.13 用法。
fn generate_selfsigned(domain: &str) -> Result<(String, String), String> {
    let kp = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = rcgen::CertificateParams::new(vec![domain.to_string()])
        .and_then(|p| p.self_signed(&kp))
        .map_err(|e| e.to_string())?;
    Ok((cert.pem().to_string(), kp.serialize_pem()))
}

/// 生成并持久化自签证书。返回 (cert_path, key_path)。
fn ensure_selfsigned(cfg: &Cfg) -> Result<(String, String), String> {
    let d = home_dir().join("etc").join("https-front").join("selfsigned");
    let _ = std::fs::create_dir_all(&d);
    let cert_path = d.join("cert.pem");
    let key_path = d.join("key.pem");
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path.to_string_lossy().into_owned(), key_path.to_string_lossy().into_owned()));
    }
    let (cert, key) = generate_selfsigned(&cfg.domain)?;
    std::fs::write(&cert_path, &cert).map_err(|e| e.to_string())?;
    std::fs::write(&key_path, &key).map_err(|e| e.to_string())?;
    Ok((cert_path.to_string_lossy().into_owned(), key_path.to_string_lossy().into_owned()))
}

/// 组装 rustls ServerConfig（ring provider）。
fn make_server_config(cfg: &Cfg) -> Result<Arc<rustls::ServerConfig>, String> {
    let (cert_file, key_file) = if cfg.mode == "cert" && !cfg.cert_file.is_empty() && !cfg.key_file.is_empty() {
        (cfg.cert_file.clone(), cfg.key_file.clone())
    } else {
        // selfsigned 或 acme(回退 selfsigned)
        ensure_selfsigned(cfg)?
    };

    let certs = load_certs(&cert_file)?;
    let key = load_key(&key_file)?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let scfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("协议版本: {}", e))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("加载证书: {}", e))?;
    Ok(Arc::new(scfg))
}

fn load_certs(path: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let data = std::fs::read(path).map_err(|e| format!("读证书 {}: {}", path, e))?;
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(&data[..]))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("解析证书: {}", e))?;
    Ok(certs)
}

fn load_key(path: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let data = std::fs::read(path).map_err(|e| format!("读私钥 {}: {}", path, e))?;
    rustls_pemfile::private_key(&mut std::io::BufReader::new(&data[..]))
        .map_err(|e| format!("解析私钥: {}", e))?
        .ok_or_else(|| format!("私钥文件无有效密钥: {}", path))
}

// ---------- 反向代理（天然 HTTP）----------

/// 处理一个 TLS 连接：完成握手后读请求、转发到面板、回写响应。
fn handle_tls(mut stream: TcpStream, scfg: Arc<rustls::ServerConfig>, panel_addr: String) {
    let conn = rustls::ServerConnection::new(scfg);
    let mut tls = match conn {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut tls_stream = rustls::Stream::new(&mut tls, &mut stream);
    // 读请求头
    let req_bytes = match read_http_request(&mut tls_stream) {
        Some(b) => b,
        None => return,
    };
    // 转发到面板
    match TcpStream::connect(panel_addr.as_str()) {
        Ok(mut upstream) => {
            upstream.set_read_timeout(Some(Duration::from_secs(60))).ok();
            let _ = upstream.write_all(&req_bytes);
            let mut resp = Vec::new();
            let _ = upstream.read_to_end(&mut resp);
            let _ = tls_stream.write_all(&resp);
            let _ = tls_stream.flush();
        }
        Err(_) => {
            let _ = tls_stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
    let _ = tls_stream.flush();
}

/// 从 TLS 流读一个完整 HTTP 请求（请求行 + 头 + body）。
fn read_http_request(r: &mut dyn Read) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let mut head_end: Option<usize> = None;
    // 逐字节找 \r\n\r\n
    while head_end.is_none() {
        match r.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                head_end = find_sub(&buf, b"\r\n\r\n");
            }
        }
    }
    let head_end = head_end.unwrap();
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut content_len = 0usize;
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    // 已读 body 部分
    let have = buf.len().saturating_sub(head_end + 4);
    while have < content_len {
        match r.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    Some(buf)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------- 配置 / 状态 Web 页面 + API（走纯 HTTP，local，绑定 PLUGIN_PORT）----------

/// 把向外展示的 "listen"（形如 ":8443"）解析成端口号。
fn parse_listen(v: &str) -> u16 {
    v.trim().trim_start_matches(':').parse().unwrap_or(443)
}

fn env_path() -> std::path::PathBuf {
    home_dir().join("etc").join(".env")
}

/// 读取面板 .env 中的关键项，判断目前面板对外暴露情况。
fn panel_setup_status() -> (String, bool, bool) {
    let env = util::parse_env_file(&env_path());
    let listen = env.get("LISTEN_ADDR").cloned().unwrap_or_else(|| ":8787".into());
    // 是否仅本机可达
    let exposed = !(listen.contains("127.0.0.1")
        || listen.starts_with("127.")
        || listen.starts_with("localhost")
        || listen.contains("::1"));
    let trust_proxy = env.get("PANEL_TRUST_PROXY").map(|v| v == "1").unwrap_or(false);
    (listen, exposed, trust_proxy)
}

fn api_get_config(cfg: &Cfg) -> Response {
    Response::json(&serde_json::json!({
        "mode": cfg.mode,
        "panel_addr": cfg.panel_addr,
        "listen": format!(":{}", cfg.https_port),
        "cert_file": cfg.cert_file,
        "key_file": cfg.key_file,
        "domain": cfg.domain,
        "email": cfg.email,
        "acme_http_addr": cfg.acme_http_addr,
    }))
}

fn api_get_status(cfg: &Cfg) -> Response {
    let (panel_listen, panel_exposed, panel_trust_proxy) = panel_setup_status();
    // 简化证书到期展示：自签长期有效，cert 模式暂不解析具体日期
    let cert_expiry = match cfg.mode.as_str() {
        "selfsigned" | "acme" => "自签（长期有效）".to_string(),
        _ => "-".to_string(),
    };
    Response::json(&serde_json::json!({
        "mode": cfg.mode,
        "listen": format!(":{}", cfg.https_port),
        "cert_expiry": cert_expiry,
        "panel_listen": panel_listen,
        "panel_exposed": panel_exposed,
        "panel_trust_proxy": panel_trust_proxy,
    }))
}

fn api_post_config(req: &Request) -> Response {
    let body: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => return Response::json_err(400, "请求体不是有效 JSON"),
    };
    let mut cfg = load_cfg();
    if let Some(v) = body["mode"].as_str() {
        cfg.mode = v.to_string();
    }
    if let Some(v) = body["panel_addr"].as_str() {
        if !v.trim().is_empty() {
            cfg.panel_addr = v.trim().to_string();
        }
    }
    if let Some(v) = body["listen"].as_str() {
        cfg.https_port = parse_listen(v);
    }
    if let Some(v) = body["cert_file"].as_str() {
        cfg.cert_file = v.trim().to_string();
    }
    if let Some(v) = body["key_file"].as_str() {
        cfg.key_file = v.trim().to_string();
    }
    if let Some(v) = body["domain"].as_str() {
        if !v.trim().is_empty() {
            cfg.domain = v.trim().to_string();
        }
    }
    if let Some(v) = body["email"].as_str() {
        cfg.email = v.trim().to_string();
    }
    if let Some(v) = body["acme_http_addr"].as_str() {
        cfg.acme_http_addr = v.trim().to_string();
    }
    if let Err(e) = save_cfg(&cfg) {
        return Response::json_err(500, &format!("保存配置失败: {}", e));
    }
    Response::json(&serde_json::json!({ "ok": true }))
}

/// 一键安全设置：把面板改为仅本机监听并开启受信反代。
fn api_panel_setup() -> Response {
    let p = env_path();
    if let Err(e) = util::set_env_var(&p, "LISTEN_ADDR", "127.0.0.1:8787") {
        return Response::json_err(500, &format!("写入面板配置失败: {}", e));
    }
    if let Err(e) = util::set_env_var(&p, "PANEL_TRUST_PROXY", "1") {
        return Response::json_err(500, &format!("写入面板配置失败: {}", e));
    }
    Response::json(&serde_json::json!({ "ok": true }))
}

fn http_handle(req: &Request) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET" | "HEAD", "/api/config") => {
            let cfg = load_cfg();
            api_get_config(&cfg)
        }
        ("GET" | "HEAD", "/api/status") => {
            let cfg = load_cfg();
            api_get_status(&cfg)
        }
        ("POST", "/api/config") => api_post_config(req),
        ("POST", "/api/panel-setup") => api_panel_setup(),
        // 根与任何子路径都回配置页面
        _ => {
            let mut r = Response::html(INDEX_HTML);
            r.headers.push(("Cache-Control".into(), "no-cache".into()));
            r
        }
    }
}

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port_env = std::env::var("PLUGIN_PORT").ok().and_then(|p| p.parse::<u16>().ok());
    let cfg = load_cfg();
    let https_port = cfg.https_port;

    // ---- 1) HTTP 配置/状态服务：面板网关用普通 HTTP 经 /p/https-front/ 反代到此，
    //        故必须绑定面板注入的 PLUGIN_PORT。----
    let http_port = port_env.unwrap_or(19000);
    let hb = bind.clone();
    let http_thread = std::thread::spawn(move || {
        let handler = |req: &Request| http_handle(req);
        eprintln!("[https-front] 面板配置服务 listening on {}:{}", hb, http_port);
        if let Err(e) = iotapanel_sdk::http::serve(&hb, http_port, handler) {
            eprintln!("[https-front] 配置服务启动失败: {}", e);
            std::process::exit(1);
        }
    });

    // ---- 2) TLS 前端：真实 HTTPS 入口，绑定 config 里的 https_port。----
    let panel_addr = cfg.panel_addr.clone();
    let scfg = match make_server_config(&cfg) {
        Ok(c) => c,
        Err(e) => {
            // 证书有问题时仍保留配置服务，方便在面板页修复
            eprintln!("[https-front] TLS 证书加载失败（配置页面仍可用）：{}", e);
            let _ = http_thread.join();
            return;
        }
    };

    let abs_listen = format!("{}:{}", if bind.is_empty() { "0.0.0.0" } else { &bind }, https_port);
    match TcpListener::bind(&abs_listen) {
        Ok(listener) => {
            eprintln!(
                "[https-front] HTTPS listening on {} (mode={}, panel_addr={})",
                abs_listen, cfg.mode, panel_addr
            );
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let scfg = scfg.clone();
                let panel_addr = panel_addr.clone();
                std::thread::spawn(move || {
                    stream.set_nodelay(true).ok();
                    handle_tls(stream, scfg, panel_addr);
                });
            }
        }
        Err(e) => {
            eprintln!(
                "[https-front] TLS 监听失败 {}: {}（保留配置服务，请调整对外 HTTPS 端口）",
                abs_listen, e
            );
            let _ = http_thread.join();
        }
    }
}