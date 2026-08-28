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

//! IotaPanel 面板核心入口。
//!
//! 极简微内核：认证、反向代理网关、插件进程管理；功能全部在插件里。
//! WebSocket 升级请求在本层做字节级双向桥接（终端等插件依赖）。

mod auth;
mod config;
mod db;
mod embed;
mod embedded_data;
mod gateway;
mod installer;
mod manifest;
mod manager;
mod server;
mod util;

use iotapanel_sdk::http as mini_http;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(cmd) = args.get(1) {
        match cmd.as_str() {
            "-v" | "--version" | "-version" | "version" => {
                println!("IotaPanel {}", config::VERSION);
                return;
            }
            "start" | "stop" | "restart" | "uninstall" | "status" | "log"
            | "help" | "-h" | "--help" => {
                cli::run(&args[1..]);
                return;
            }
            "serve" => {
                // 前台守护启动：继续进入 serve()
            }
            other => {
                println!("未知命令: {}\n", other);
                cli::print_help();
                std::process::exit(2);
            }
        }
    }
    serve();
}

fn serve() {
    let mut cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("加载配置失败: {}", e);
            std::process::exit(1);
        }
    };
    if let Err(e) = cfg.ensure_secret() {
        eprintln!("{}", e);
        std::process::exit(1);
    }

    // 目录准备 + 核心日志轮转（>20MB 改名 .1）
    let log_dir = std::path::Path::new(&cfg.home).join("logs");
    let _ = std::fs::create_dir_all(log_dir.join("plugins"));
    let panel_log = log_dir.join("panel.log");
    if let Ok(md) = std::fs::metadata(&panel_log) {
        if md.len() > 20 << 20 {
            let _ = std::fs::rename(&panel_log, log_dir.join("panel.log.1"));
        }
    }

    manager::LOG_CONTEXT.set_home(&cfg.home);
    manager::log_line(
        "INFO",
        &format!(
            "IotaPanel 启动 version={} addr={} home={} idle_timeout={}s port_pool={}-{}",
            config::VERSION,
            cfg.listen_addr,
            cfg.home,
            cfg.idle_timeout_secs,
            cfg.port_lo,
            cfg.port_hi
        ),
    );

    let database = Arc::new(match db::Db::open(&cfg.home) {
        Ok(d) => d,
        Err(e) => {
            manager::log_line("ERROR", &format!("初始化数据库失败 err={}", e));
            std::process::exit(1);
        }
    });
    // 设置页持久化的空闲退出时间优先于 .env
    if let Some(mins) = database.get_setting("idle_timeout_minutes") {
        if let Ok(m) = mins.parse::<u64>() {
            if m > 0 && m <= 1440 {
                cfg.idle_timeout_secs = m * 60;
            }
        }
    }

    // 拷贝即安装：扫描 plugins/ 自动登记新目录
    sync_plugins_from_dir(&cfg.home, &database);

    let mgr = manager::Manager::new(
        &cfg.home.clone(),
        cfg.idle_timeout_secs,
        cfg.port_lo,
        cfg.port_hi,
        database.clone(),
    );
    mgr.load_adopt(); // 认领仍存活的插件进程 + 保活自愈

    let srv = server::Server::new(cfg, database.clone(), mgr.clone());

    // 后台：空闲退出扫描（5 秒周期，事件驱动 touch 重置）
    {
        let mgr2 = mgr.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            if SHUTDOWN.load(Ordering::Relaxed) {
                return;
            }
            mgr2.sweep_idle();
        });
    }

    // 先把监听地址取出来再 bind：若在 match scrutinee 里 lock 而 bind 失败，
    // Err 分支再次 lock 同一个 Mutex 会自死锁（Rust 中 scrutinee 临时守卫存活到 match 结束）。
    let listen_addr = srv.cfg.lock().unwrap().listen_addr.clone();
    let listener = match bind_listener(&listen_addr) {
        Ok(l) => l,
        Err(e) => {
            manager::log_line("ERROR", &format!("监听失败 addr={} err={}", listen_addr, e));
            std::process::exit(1);
        }
    };
    manager::log_line("INFO", "面板就绪");

    unsafe {
        libc_signal(SIGTERM, handle_signal);
        libc_signal(SIGINT, handle_signal);
    }

    // 非阻塞 accept + 轮询：保证收到 SIGTERM/SIGINT 后能及时退出。
    // （旧版阻塞在 accept() 上，信号只置标志不唤醒，进程不退出、端口一直占用，
    //   导致后续重启 bind 失败并触发上面的 Mutex 自死锁，形成级联故障。）
    listener.set_nonblocking(true).ok();
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let srv = srv.clone();
                std::thread::spawn(move || {
                    stream.set_nodelay(true).ok();
                    stream.set_read_timeout(Some(Duration::from_secs(75))).ok();
                    handle_connection(stream, srv);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => {
                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    // graceful shutdown：只停非保活插件（保活插件跨重启复用）
    manager::log_line("INFO", "收到退出信号，开始清理");
    mgr.shutdown();
    manager::log_line("INFO", "面板核心已退出");
}

extern "C" fn handle_signal(_sig: i32) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

unsafe fn libc_signal(sig: i32, handler: extern "C" fn(i32)) {
    unsafe extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    signal(sig, handler);
}

const SIGTERM: i32 = 15;
const SIGINT: i32 = 2;

/// 监听地址：":8787" 全网卡双栈 / "0.0.0.0:8787" 仅 IPv4 / "127.0.0.1:8787" 本机。
pub fn bind_listener(addr: &str) -> std::io::Result<TcpListener> {
    if let Some(port) = addr.strip_prefix(':') {
        if let Ok(l) = TcpListener::bind(format!("[::]:{}", port)) {
            return Ok(l); // IPv6 双栈 socket 通常同时接受 IPv4-mapped
        }
        return TcpListener::bind(format!("0.0.0.0:{}", port));
    }
    TcpListener::bind(addr)
}

/// 单连接处理：读首个请求；WS 升级进桥接模式，其余走 handler（Keep-Alive）。
fn handle_connection(stream: TcpStream, srv: Arc<server::Server>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let req = match mini_http::read_request(&mut reader, mini_http::DEFAULT_MAX_BODY) {
        Ok(Some(r)) => r,
        _ => return,
    };

    if is_ws_upgrade(&req) {
        if let Some((name, plugin_path)) = ws_gateway_target(&req) {
            drop(reader);
            ws_bridge(stream, &req, &name, &plugin_path, &srv);
            return;
        }
    }

    // Keep-Alive 循环
    let mut current = req;
    loop {
        if is_ws_upgrade(&current) {
            if let Some((name, plugin_path)) = ws_gateway_target(&current) {
                drop(reader);
                ws_bridge(stream, &current, &name, &plugin_path, &srv);
                return;
            }
        }
        let resp = server::with_security_headers(srv.handle(&current));
        let close = matches!(
            current.header("connection").map(|c| c.to_ascii_lowercase()),
            Some(c) if c.contains("close")
        );
        {
            let mut writer = match reader.get_ref().try_clone() {
                Ok(w) => w,
                Err(_) => return,
            };
            if mini_http::write_response(&mut writer, &resp, current.method == "HEAD", close).is_err() {
                return;
            }
        }
        if close {
            return;
        }
        match mini_http::read_request(&mut reader, mini_http::DEFAULT_MAX_BODY) {
            Ok(Some(next)) => current = next,
            _ => return,
        }
    }
}

fn is_https_via_proxy(req: &mini_http::Request) -> bool {
    req.header("x-forwarded-proto")
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

fn is_ws_upgrade(req: &mini_http::Request) -> bool {
    req.header("upgrade")
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
        && req
            .header("connection")
            .map(|c| c.to_ascii_lowercase().contains("upgrade"))
            .unwrap_or(false)
}

fn ws_gateway_target(req: &mini_http::Request) -> Option<(String, String)> {
    let path = req.path.strip_prefix("/p/")?;
    let (name, rest) = path.split_once('/')?;
    if !valid_plugin_name(name) {
        return None;
    }
    Some((name.to_string(), format!("/{}", rest)))
}

fn valid_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

/// WebSocket 字节级双向桥接：转发握手头 → 收到上游响应头后裸拷贝两个方向。
fn ws_bridge(
    client: TcpStream,
    req: &mini_http::Request,
    name: &str,
    plugin_path: &str,
    srv: &Arc<server::Server>,
) {
    let home = srv.cfg.lock().unwrap().home.clone();
    let mf_path = std::path::Path::new(&home).join("plugins").join(name);
    let auth_none =
        manifest::Manifest::load(&mf_path).map(|m| m.auth == "none").unwrap_or(false);
    // 与普通网关一致：auth:none 且 /mcp 免登录（MCP 客户端直连）；页面仍需登录
    let exempt = auth_none && plugin_path.starts_with("/mcp");
    if !exempt && !srv.logged_in(req) {
        let mut w = client;
        let _ = mini_http::write_response(
            &mut w,
            &mini_http::Response::json_err(401, "未登录"),
            false,
            true,
        );
        return;
    }
    if srv.manager.start(name).is_err() {
        let mut w = client;
        let _ = mini_http::write_response(
            &mut w,
            &mini_http::Response::json_err(502, "插件启动失败"),
            false,
            true,
        );
        return;
    }
    srv.manager.touch(name);
    let Some((port, bind)) = srv.manager.runtime_of(name) else { return };
    let bind_addr = if bind.contains(':') { format!("[{}]", bind) } else { bind };
    let Ok(upstream) = TcpStream::connect(format!("{}:{}", bind_addr, port)) else {
        let mut w = client;
        let _ = mini_http::write_response(
            &mut w,
            &mini_http::Response::json_err(502, "插件连接失败"),
            false,
            true,
        );
        return;
    };

    // 组装上游握手（保留 Sec-WebSocket-* 全部头）
    let target = match req.target.split_once('?') {
        Some((_, q)) => format!("{}?{}", plugin_path, q),
        None => plugin_path.to_string(),
    };
    let orig_host = req.header("host").unwrap_or("").to_string();
    let mut head = String::new();
    head.push_str(&format!("GET {} HTTP/1.1\r\n", target));
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("host")
            || k.eq_ignore_ascii_case("connection")
            || k.eq_ignore_ascii_case("upgrade")
            || k.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str(&format!("Host: {}\r\n", orig_host));
    head.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\n");
    head.push_str("X-Forwarded-Proto: http\r\n");
    head.push_str(&format!("X-Forwarded-Host: {}\r\n", orig_host));
    head.push_str(&format!("X-Panel-Plugin: {}\r\n\r\n", name));

    let mut upstream = upstream;
    if upstream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let _ = upstream.flush();

    // 双向裸拷贝（101 之后就是纯隧道）
    let mut client_r = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut client_w = client;
    let mut up_r = match upstream.try_clone() {
        Ok(u) => u,
        Err(_) => return,
    };
    let mut up_w = upstream;

    let c2u = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match client_r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if up_w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = up_w.shutdown(std::net::Shutdown::Both);
    });
    let u2c = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match up_r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client_w.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = client_w.shutdown(std::net::Shutdown::Both);
    });
    let _ = c2u.join();
    let _ = u2c.join();
}

/// 扫描 PANEL_HOME/plugins/ 自动登记未入库的插件目录（拷贝即安装）。
fn sync_plugins_from_dir(home: &str, database: &Arc<db::Db>) {
    let Ok(rd) = std::fs::read_dir(std::path::Path::new(home).join("plugins")) else {
        return;
    };
    for e in rd.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if database.get_plugin(&name).is_some() {
            continue;
        }
        let mf = match manifest::Manifest::load(&e.path()) {
            Ok(m) => m,
            Err(_) => {
                manager::log_line("WARN", &format!("跳过无法识别的插件目录 plugin={}", name));
                continue;
            }
        };
        let _ = database.upsert_plugin(db::PluginRecord {
            name: mf.name.clone(),
            title: mf.title.clone(),
            version: mf.version.clone(),
            author: mf.author.clone(),
            description: mf.description.clone(),
            keepalive: mf.keepalive,
            installed_at: util::rfc3339_now(),
            source: "local".into(),
        });
        if mf.keepalive {
            let _ = database.set_keepalive(&mf.name, true);
        }
        manager::log_line(
            "INFO",
            &format!("自动登记插件 plugin={} version={}", mf.name, mf.version),
        );
    }
}

// ================= CLI =================

mod cli {
    const SERVICE: &str = "iotapanel";

    pub fn print_help() {
        println!(
            r#"IotaPanel 面板控制命令

用法:
  iotapanel start      启动面板（systemd 安装时走 systemctl）
  iotapanel stop       停止面板（保留保活插件进程）
  iotapanel restart    重启面板
  iotapanel status     查看面板状态
  iotapanel log        查看核心日志（iotapanel log -n 100 指定行数）
  iotapanel uninstall  卸载面板（数据保留）
  iotapanel version    显示版本
  iotapanel help       显示帮助

服务名: {SERVICE}（systemd: systemctl status {SERVICE}）"#
        );
    }

    pub fn run(args: &[String]) {
        if args.is_empty() {
            print_help();
            return;
        }
        match args[0].as_str() {
            "start" => cli_start(),
            "stop" => cli_stop(),
            "restart" => cli_restart(),
            "uninstall" => cli_uninstall(),
            "status" => cli_status(),
            "log" => cli_log(args),
            "serve" => {} // 外层继续启动服务
            "help" | "-h" | "--help" => print_help(),
            _ => print_help(),
        }
    }

    fn has_systemd() -> bool {
        std::path::Path::new(&format!("/etc/systemd/system/{}.service", SERVICE)).exists()
    }

    fn systemctl(action: &str) -> bool {
        std::process::Command::new("systemctl")
            .args([action, SERVICE])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn panel_pids() -> Vec<i32> {
        let out = std::process::Command::new("pgrep")
            .args(["-x", "iotapanel"])
            .output()
            .ok()
            .map(|o| o.stdout)
            .unwrap_or_default();
        String::from_utf8_lossy(&out)
            .split_whitespace()
            .filter_map(|w| w.parse().ok())
            .filter(|pid| *pid != std::process::id() as i32)
            .collect()
    }

    /// 安装目录：运行进程环境 > $PANEL_HOME > 二进制布局 > 标记文件。
    fn resolve_home() -> Option<String> {
        for pid in panel_pids() {
            if let Ok(data) = std::fs::read_to_string(format!("/proc/{}/environ", pid)) {
                for kv in data.split('\0') {
                    if let Some(v) = kv.strip_prefix("PANEL_HOME=") {
                        if !v.is_empty() {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
        if let Ok(h) = std::env::var("PANEL_HOME") {
            if !h.is_empty() {
                return Some(h);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                if dir.file_name().map(|n| n == "bin").unwrap_or(false) {
                    if let Some(parent) = dir.parent() {
                        return Some(parent.to_string_lossy().into_owned());
                    }
                }
            }
        }
        if let Ok(data) = std::fs::read_to_string("/tmp/iotapanel-home") {
            let home = data.trim().to_string();
            if !home.is_empty() && std::path::Path::new(&home).join("etc/.env").exists() {
                return Some(home);
            }
        }
        None
    }

    fn cli_start() {
        if has_systemd() {
            if systemctl("start") {
                println!("✅ 已启动（systemd）");
            } else {
                println!("启动失败");
                std::process::exit(1);
            }
            return;
        }
        if !panel_pids().is_empty() {
            println!("面板已在运行");
            return;
        }
        if std::env::var("PANEL_HOME").map(|v| v.is_empty()).unwrap_or(true) {
            if let Some(home) = resolve_home() {
                std::env::set_var("PANEL_HOME", home);
            }
        }
        let exe = std::env::current_exe().expect("current exe");
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/iotapanel.log")
            .expect("open log");
        let log_err = log.try_clone().expect("clone log");
        match std::process::Command::new(&exe)
            .arg("serve")
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(log_err))
            .spawn()
        {
            Ok(_) => println!("✅ 已启动（非 systemd，日志: /tmp/iotapanel.log）"),
            Err(e) => {
                println!("启动失败: {}", e);
                std::process::exit(1);
            }
        }
    }

    fn cli_stop() {
        if has_systemd() {
            if systemctl("stop") {
                println!("🛑 已停止（systemd）");
            } else {
                println!("停止失败");
                std::process::exit(1);
            }
            return;
        }
        let pids = panel_pids();
        if pids.is_empty() {
            println!("面板未在运行");
            return;
        }
        for pid in pids {
            iotapanel_sdk::util::terminate_process(pid);
            println!("🛑 已向面板进程 {} 发送停止信号", pid);
        }
    }

    fn cli_restart() {
        if has_systemd() {
            if systemctl("restart") {
                println!("✅ 已重启（systemd）");
            } else {
                println!("重启失败");
                std::process::exit(1);
            }
            return;
        }
        if let Some(home) = resolve_home() {
            std::env::set_var("PANEL_HOME", home);
        }
        cli_stop();
        std::thread::sleep(std::time::Duration::from_millis(800));
        cli_start();
    }

    fn cli_uninstall() {
        let home = resolve_home().unwrap_or_else(|| "/data/panel".into());
        println!("即将卸载 IotaPanel");
        println!("安装目录: {}", home);
        print!("确认卸载？（停止面板并移除 systemd 服务，数据目录将保留）[y/N]: ");
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let mut ans = String::new();
        let _ = std::io::stdin().read_line(&mut ans);
        let ans = ans.trim().to_ascii_lowercase();
        if ans != "y" && ans != "yes" {
            println!("已取消");
            return;
        }
        cli_stop();
        if has_systemd() {
            let _ =
                std::process::Command::new("systemctl").args(["disable", SERVICE]).status();
            let _ = std::fs::remove_file(format!("/etc/systemd/system/{}.service", SERVICE));
            let _ = std::process::Command::new("systemctl").arg("daemon-reload").status();
            println!("已移除 systemd 服务");
        }
        let _ = std::fs::remove_file("/usr/local/bin/iotapanel");
        println!(
            "✅ 已卸载。数据保留在 {}（彻底删除请执行: rm -rf {}）",
            home, home
        );
    }

    fn cli_status() {
        println!("IotaPanel {}", crate::config::VERSION);
        let home = resolve_home();
        let mut listen = std::env::var("LISTEN_ADDR").unwrap_or_default();
        if listen.is_empty() {
            if let Some(h) = &home {
                for (k, v) in
                    iotapanel_sdk::util::parse_env_file(&std::path::Path::new(h).join("etc/.env"))
                {
                    if k == "LISTEN_ADDR" {
                        listen = v;
                    }
                }
            }
        }
        if let Some(h) = &home {
            println!("安装目录: {}", h);
        }
        if !listen.is_empty() {
            println!("监听地址: {}", listen);
        }
        if has_systemd() {
            let active = std::process::Command::new("systemctl")
                .args(["is-active", SERVICE])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            println!("systemd 服务: {}", active);
        }
        let pids = panel_pids();
        if pids.is_empty() {
            println!("面板进程: 未运行");
        } else {
            let list: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
            println!("面板进程: 运行中 (PID {})", list.join(", "));
            if let Some(h) = &home {
                if let Ok(data) =
                    std::fs::read_to_string(std::path::Path::new(h).join("etc/port-map.json"))
                {
                    let n = data.matches("\"port\"").count();
                    println!("运行中插件: {} 个", n);
                }
            }
        }
    }

    fn cli_log(args: &[String]) {
        let mut n = 100usize;
        if args.len() > 2 && args[1] == "-n" {
            if let Ok(v) = args[2].parse::<usize>() {
                n = v.max(1);
            }
        }
        let Some(home) = resolve_home() else {
            println!("无法确定安装目录，请设置 PANEL_HOME");
            std::process::exit(1);
        };
        let path = std::path::Path::new(&home).join("logs/panel.log");
        let Ok(data) = std::fs::read_to_string(&path) else {
            println!("日志文件不存在: {}", path.display());
            std::process::exit(1);
        };
        let lines: Vec<&str> = data.lines().collect();
        let start = lines.len().saturating_sub(n);
        println!("{}", lines[start..].join("\n"));
    }
}
