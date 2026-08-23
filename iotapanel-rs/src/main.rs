//! IotaPanel micro-kernel (Rust rewrite).
//!
//! Minimal core: user auth + reverse-proxy gateway + plugin process manager.
//! Small idle footprint; plugins cold-start on demand and idle-exit to free
//! memory. Mirrors the original Go `cmd/panel`.

mod api;
mod auth;
mod config;
mod db;
mod embed;
mod gateway;
mod plugins;

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

/// Append-only log writer. Opened once at startup, written behind a mutex.
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn log_line(level: &str, msg: &str) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let line = format!("{} {} {}", level, ts, msg);
    eprintln!("{}", line);
    if let Ok(mut f) = LOG.lock() {
        if let Some(f) = f.as_mut() {
            let _ = f.write_all(format!("{}\n", line).as_bytes());
        }
    }
}

pub fn log_info(msg: &str) {
    log_line("INFO", msg);
}
pub fn log_warn(msg: &str) {
    log_line("WARN", msg);
}
pub fn log_error(msg: &str) {
    log_line("ERROR", msg);
}

/// Create the core log file (rotating at 20MB), mirroring the Go startup.
fn init_logging(home: &str) {
    let dir = Path::new(home).join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("panel.log");
    if let Ok(md) = std::fs::metadata(&path) {
        if md.len() > 20 << 20 {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    if let Ok(mut f) = LOG.lock() {
        *f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
    }
}

// =============================== CLI HELPERS ===============================

/// PIDs of running `panel` processes, excluding ourselves.
fn find_panel_pids() -> Vec<i32> {
    let out = Command::new("pgrep").args(["-x", "panel"]).output().ok();
    let out = match out {
        Some(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let self_pid = std::process::id();
    stdout
        .split_whitespace()
        .filter_map(|f| f.parse::<i32>().ok())
        .filter(|pid| *pid != self_pid as i32)
        .collect()
}

/// Environment of a running panel process (first one found).
fn running_panel_env() -> Option<std::collections::HashMap<String, String>> {
    let pids = find_panel_pids();
    if pids.is_empty() {
        return None;
    }
    let data = std::fs::read(format!("/proc/{}/environ", pids[0])).ok()?;
    let mut env = std::collections::HashMap::new();
    for kv in data.split(|&b| b == 0) {
        let kv = String::from_utf8_lossy(kv);
        let mut it = kv.splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            env.insert(k.to_string(), v.to_string());
        }
    }
    Some(env)
}

fn has_systemd() -> bool {
    Path::new("/etc/systemd/system/iotapanel.service").exists()
}

fn run_systemctl(action: &str) {
    let status = Command::new("systemctl").args([action, "iotapanel"]).status();
    match status {
        Ok(s) if s.success() => {}
        _ => println!("systemctl {} 失败", action),
    }
}

/// Determine install dir: running process env > PANEL_HOME > exe location.
fn resolve_home() -> String {
    if let Some(env) = running_panel_env() {
        if let Some(h) = env.get("PANEL_HOME") {
            if !h.is_empty() {
                return h.clone();
            }
        }
    }
    if let Ok(v) = std::env::var("PANEL_HOME") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            if parent.file_name().map(|s| s == "bin").unwrap_or(false) {
                if let Some(dir) = parent.parent() {
                    return dir.to_string_lossy().to_string();
                }
            }
        }
    }
    "/data/panel".to_string()
}

/// Tail the panel log like Go `cliLog`.
fn cli_log(args: &[String]) {
    let mut n = 100usize;
    if args.len() > 2 && args[1] == "-n" {
        if let Ok(v) = args[2].parse::<usize>() {
            if v > 0 {
                n = v;
            }
        }
    }
    let home = resolve_home();
    let path = Path::new(&home).join("logs").join("panel.log");
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => {
            println!("日志文件不存在: {}", path.display());
            return;
        }
    };
    let mut lines: Vec<&str> = data.trim_end_matches('\n').split('\n').collect();
    if lines.len() > n {
        lines.drain(..lines.len() - n);
    }
    println!("{}", lines.join("\n"));
}

fn print_help() {
    println!(
        "IotaPanel 面板控制命令\n\n\
         用法:\n\
          panel start      启动面板（systemd 安装时走 systemctl）\n\
          panel stop       停止面板（保留保活插件进程）\n\
          panel restart    重启面板\n\
          panel status     查看面板状态（进程/端口/插件）\n\
          panel log        查看核心日志（panel log -n 100 指定行数）\n\
          panel uninstall  卸载面板（停止服务、移除 systemd 与命令，数据保留）\n\
          panel version    显示版本\n\
          panel help       显示帮助\n\n\
         服务名: iotapanel（systemd: systemctl status iotapanel）"
    );
}

fn cli_start() {
    if has_systemd() {
        run_systemctl("start");
        println!("已启动（systemd）");
        return;
    }
    if !find_panel_pids().is_empty() {
        println!("面板已在运行");
        return;
    }
    if std::env::var("PANEL_HOME").unwrap_or_default().is_empty() {
        let home = resolve_home();
        std::env::set_var("PANEL_HOME", &home);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/iotapanel.log")
        .ok();
    let mut cmd = Command::new("nohup");
    cmd.arg(&exe).arg("serve");
    cmd.envs(std::env::vars());
    if let Some(f) = &log_file {
        if let (Ok(stdout), Ok(stderr)) = (f.try_clone(), f.try_clone()) {
            cmd.stdout(stdout).stderr(stderr);
        }
    }
    match cmd.spawn() {
        Ok(_) => println!("已启动（非 systemd，日志: /tmp/iotapanel.log）"),
        Err(e) => println!("启动失败: {}", e),
    }
}

fn cli_stop() {
    if has_systemd() {
        run_systemctl("stop");
        println!("已停止（systemd）");
        return;
    }
    let pids = find_panel_pids();
    if pids.is_empty() {
        println!("面板未在运行");
        return;
    }
    for pid in pids {
        if Command::new("kill").args(["-TERM", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false) {
            println!("已向面板进程 {} 发送停止信号", pid);
        }
    }
}

fn cli_restart() {
    if has_systemd() {
        run_systemctl("restart");
        println!("已重启（systemd）");
        return;
    }
    let home = resolve_home();
    std::env::set_var("PANEL_HOME", &home);
    cli_stop();
    std::thread::sleep(Duration::from_millis(800));
    cli_start();
}

fn cli_status() {
    println!("IotaPanel {}", config::VERSION);
    let home = resolve_home();
    let env = running_panel_env();
    let listen = env
        .as_ref()
        .and_then(|e| e.get("LISTEN_ADDR").cloned())
        .unwrap_or_default();
    println!("安装目录: {}", home);
    if listen.is_empty() {
        let env_path = Path::new(&home).join("etc").join(".env");
        if let Ok(data) = std::fs::read_to_string(&env_path) {
            for line in data.lines() {
                if let Some(v) = line.trim().strip_prefix("LISTEN_ADDR=") {
                    let listen = v.trim().trim_matches(['"', '\'']).to_string();
                    println!("监听地址: {}", listen);
                    break;
                }
            }
        }
    } else {
        println!("监听地址: {}", listen);
    }
    if has_systemd() {
        if let Ok(out) = Command::new("systemctl").args(["is-active", "iotapanel"]).output() {
            println!("systemd 服务: {}", String::from_utf8_lossy(&out.stdout).trim());
        }
    }
    let pids = find_panel_pids();
    if pids.is_empty() {
        println!("面板进程: 未运行");
        return;
    }
    let plist: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
    println!("面板进程: 运行中 (PID {})", plist.join(", "));
    let port_map = Path::new(&home).join("etc").join("port-map.json");
    if let Ok(data) = std::fs::read_to_string(&port_map) {
        let running = data.matches("\"port\"").count();
        println!("运行中插件: {} 个", running);
    }
}

fn cli_uninstall() {
    let home = resolve_home();
    println!("即将卸载 IotaPanel");
    println!("安装目录: {}", home);
    print!("确认卸载？（停止面板并移除 systemd 服务，数据目录将保留）[y/N]: ");
    let _ = std::io::stdout().flush();
    let mut ans = String::new();
    let _ = std::io::stdin().read_line(&mut ans);
    let a = ans.trim().to_ascii_lowercase();
    if a != "y" && a != "yes" {
        println!("已取消");
        return;
    }
    cli_stop();
    if has_systemd() {
        run_systemctl("stop");
        let _ = Command::new("systemctl").args(["disable", "iotapanel"]).status();
        let _ = std::fs::remove_file("/etc/systemd/system/iotapanel.service");
        let _ = Command::new("systemctl").arg("daemon-reload").status();
        println!("已移除 systemd 服务");
    }
    let _ = std::fs::remove_file("/usr/local/bin/panel");
    println!("已卸载。数据保留在 {}（彻底删除请执行: rm -rf {}）", home, home);
}

// =============================== SERVER ===============================

/// Normalize a `:8787` / `127.0.0.1:8787` / `[::]:8787` listen address.
fn listener_addr(listen: &str) -> String {
    let s = listen.trim_matches(['"', '\'']);
    if let Some(rest) = s.strip_prefix(':') {
        format!("0.0.0.0:{}", rest)
    } else if let Some(rest) = s.strip_prefix("[::]:") {
        format!("[::]:{}", rest)
    } else {
        s.to_string()
    }
}

/// Scan `PANEL_HOME/plugins/`, registering unrecorded plugin dirs (copy-to-install).
fn sync_plugins_from_dir(home: &str, database: &db::Db) {
    let dir = Path::new(home).join("plugins");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return, // not yet installed
    };
    for e in entries.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if database.is_installed(&name) {
            continue;
        }
        let mf = match plugins::load_manifest(&e.path()) {
            Ok(m) => m,
            Err(_) => {
                log_warn(&format!("跳过无法识别的插件目录（缺少有效 manifest.yaml）: {}", name));
                continue;
            }
        };
        if database
            .upsert_plugin(db::PluginRecord {
                name: mf.name.clone(),
                title: mf.title.clone(),
                version: mf.version.clone(),
                author: mf.author.clone(),
                description: mf.description.clone(),
                keepalive: false,
                installed_at: db::now(),
                source: "local".to_string(),
            })
            .is_err()
        {
            log_warn(&format!("登记插件失败: {}", name));
            continue;
        }
        if mf.keepalive {
            let _ = database.set_keepalive(&name, true);
        }
        log_info(&format!("自动登记插件（拷贝即安装）: {} v{}", name, mf.version));
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn serve() -> Result<(), String> {
    let cfg = config::load()?;
    init_logging(&cfg.home);

    let database = db::Db::open(&cfg.home)?;
    let database = std::sync::Arc::new(database);

    let store: std::sync::Arc<dyn plugins::Store> = database.clone();
    let mgr = plugins::Manager::new(&cfg.home, cfg.idle_timeout, cfg.port_lo, cfg.port_hi, store);
    let mgr = std::sync::Arc::new(mgr);

    // Adopt existing port-map entries + revive keepalive plugins; then register
    // any unrecorded dirs under plugins/.
    sync_plugins_from_dir(&cfg.home, &database);
    mgr.load();

    log_info(&format!(
        "IotaPanel {} starting, home={} listen={} idle={}s",
        config::VERSION,
        cfg.home,
        cfg.listen_addr,
        cfg.idle_timeout.as_secs()
    ));

    // Warm embedded binaries? No: plugins are installed lazily from bundles.

    let srv = api::Server::new(cfg.clone(), database.clone(), mgr.clone());
    let router = api::build_router(srv.clone());

    let addr = listener_addr(&cfg.listen_addr);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("监听 {} 失败: {}", addr, e))?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    log_info(&format!("面板已启动，监听 http://{}", local));

    let server = axum::serve(listener, router).with_graceful_shutdown(shutdown_signal());
    server.await.map_err(|e| format!("服务器错误: {}", e))?;

    // Graceful stop: leave keepalive plugin processes alive, close DB.
    mgr.shutdown();
    database.close().ok();
    log_info("panel exited cleanly");
    Ok(())
}

// =============================== ENTRY ===============================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = args.get(1).map(|s| s.as_str());

    match arg {
        Some("-version" | "--version" | "-v" | "version") => {
            println!("IotaPanel {}", config::VERSION);
            std::process::exit(0);
        }
        Some("start") => {
            cli_start();
            std::process::exit(0);
        }
        Some("stop") => {
            cli_stop();
            std::process::exit(0);
        }
        Some("restart") => {
            cli_restart();
            std::process::exit(0);
        }
        Some("uninstall") => {
            cli_uninstall();
            std::process::exit(0);
        }
        Some("status") => {
            cli_status();
            std::process::exit(0);
        }
        Some("log") => {
            cli_log(&args);
            std::process::exit(0);
        }
        Some("help" | "-h" | "--help") => {
            print_help();
            std::process::exit(0);
        }
        Some("serve") => {} // foreground server; fall through to tokio runtime
        Some(other) => {
            eprintln!("未知命令: {}", other);
            print_help();
            std::process::exit(2);
        }
        None => {} // default: run the server in the foreground, same as Go
    }

    let rt = tokio::runtime::Runtime::new().expect("创建异步运行时失败");
    if let Err(e) = rt.block_on(serve()) {
        log_error(&e);
        std::process::exit(1);
    }
}