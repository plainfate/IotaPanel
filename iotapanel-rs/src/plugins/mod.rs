//! Plugin lifecycle management: cold start, port pool, idle exit, keepalive,
//! port-map persistence. Faithfully mirrors the original Go `internal/plugins`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub mod install;
pub mod process;

pub const READINESS_TIMEOUT: Duration = Duration::from_secs(6);
pub const MAX_LOG_BYTES: u64 = 20 << 20;

/// Persistence interface the manager depends on (implemented by `db::Db`).
pub trait Store: Sync + Send {
    fn is_installed(&self, name: &str) -> bool;
    fn is_keepalive(&self, name: &str) -> bool;
    fn set_keepalive(&self, name: &str, v: bool) -> Result<(), String>;
}

impl Store for crate::db::Db {
    fn is_installed(&self, name: &str) -> bool {
        crate::db::Db::is_installed(self, name)
    }
    fn is_keepalive(&self, name: &str) -> bool {
        crate::db::Db::is_keepalive(self, name)
    }
    fn set_keepalive(&self, name: &str, v: bool) -> Result<(), String> {
        crate::db::Db::set_keepalive(self, name, v)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortMapEntry {
    pub port: i32,
    pub pid: i32,
    #[serde(rename = "started_at")]
    pub started_at: String,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub running: bool,
    pub port: i32,
    pub pid: i32,
    pub started_at: String,
}

/// A running plugin process. Access from concurrent threads is safe.
pub struct Runtime {
    pub name: String,
    pub port: i32,
    pub pid: i32,
    pub bind: String,
    pub start_tick: u64,
    pub started_at: String,
    last_use: AtomicU64,         // unix nanos
    stop_idle: Arc<AtomicBool>,  // stop flag for the idle-exit thread
}

impl Runtime {
    pub fn port(&self) -> i32 {
        self.port
    }
    pub fn pid(&self) -> i32 {
        self.pid
    }
    pub fn bind(&self) -> &str {
        &self.bind
    }
    fn mark_used(&self) {
        self.last_use.store(now_nanos(), Ordering::SeqCst);
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub bind: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub keepalive: bool,
    #[serde(default)]
    pub menus: Vec<Menu>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Menu {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub section: String,
}

/// Load and validate a `manifest.yaml` from a plugin install directory.
pub fn load_manifest(dir: &Path) -> Result<Manifest, String> {
    let data = std::fs::read_to_string(dir.join("manifest.yaml"))
        .map_err(|_| "读取 manifest.yaml 失败".to_string())?;
    let mut mf: Manifest = serde_yaml::from_str(&data).map_err(|e| format!("解析 manifest.yaml 失败: {}", e))?;
    if mf.name.is_empty() {
        return Err("manifest.yaml 缺少 name".into());
    }
    if mf.command.is_empty() {
        return Err("manifest.yaml 缺少 command".into());
    }
    if mf.bind.is_empty() {
        mf.bind = "127.0.0.1".to_string();
    }
    if mf.title.is_empty() {
        mf.title = mf.name.clone();
    }
    Ok(mf)
}

/// Shared mutable state, ownable by background threads via `Arc`.
struct Shared {
    mu: Mutex<HashMap<String, Arc<Runtime>>>,
    port_map_path: PathBuf,
}

impl Shared {
    fn save_port_map(&self, rt: &HashMap<String, Arc<Runtime>>) {
        let mut out = HashMap::new();
        for (name, r) in rt {
            out.insert(name.clone(), PortMapEntry { port: r.port, pid: r.pid, started_at: r.started_at.clone() });
        }
        let json = match serde_json::to_string_pretty(&out) {
            Ok(j) => j,
            Err(_) => return,
        };
        if let Some(p) = self.port_map_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let tmp = self.port_map_path.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.port_map_path);
        }
    }
}

pub struct Manager {
    pub home: String,
    idle: Mutex<Duration>,
    pub port_lo: i32,
    pub port_hi: i32,
    store: Arc<dyn Store>,
    shared: Arc<Shared>,
}

impl Manager {
    pub fn new(home: &str, idle: Duration, port_lo: i32, port_hi: i32, store: Arc<dyn Store>) -> Manager {
        Manager {
            home: home.to_string(),
            idle: Mutex::new(idle),
            port_lo,
            port_hi,
            store,
            shared: Arc::new(Shared {
                mu: Mutex::new(HashMap::new()),
                port_map_path: PathBuf::from(home).join("etc").join("port-map.json"),
            }),
        }
    }

    pub fn idle_duration(&self) -> Duration {
        *self.idle.lock().unwrap()
    }

    pub fn set_idle(&self, d: Duration) {
        *self.idle.lock().unwrap() = d;
    }

    /// Scan `port-map.json`: adopt still-listening processes, drop stale
    /// entries, then revive keepalive plugins that aren't running.
    pub fn load(&self) {
        let entries = self.read_port_map();
        {
            let mut rt = self.shared.mu.lock().unwrap();
            for (name, e) in entries {
                let bind = match load_manifest(&PathBuf::from(&self.home).join("plugins").join(&name)) {
                    Ok(mf) if !mf.bind.is_empty() => mf.bind.clone(),
                    _ => "127.0.0.1".to_string(),
                };
                if e.port <= 0 || !is_listening(&bind, e.port) {
                    continue;
                }
                let rt_arc = Arc::new(Runtime {
                    name: name.clone(),
                    port: e.port,
                    pid: e.pid,
                    bind,
                    start_tick: process::proc_start_tick(e.pid).unwrap_or(0),
                    started_at: chrono::Utc::now().to_rfc3339(),
                    last_use: AtomicU64::new(now_nanos()),
                    stop_idle: Arc::new(AtomicBool::new(false)),
                });
                if !self.store.is_keepalive(&name) {
                    self.arm_idle(rt_arc.clone());
                }
                rt.insert(name, rt_arc);
            }
            self.shared.save_port_map(&rt);
        }
        self.revive_keepalive();
    }

    fn revive_keepalive(&self) {
        let dir = PathBuf::from(&self.home).join("plugins");
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if !self.store.is_keepalive(&name) {
                continue;
            }
            let running = self.shared.mu.lock().unwrap().contains_key(&name);
            if running {
                continue;
            }
            let _ = self.start(&name);
        }
    }

    /// Cold-start a plugin process and wait for its port to open.
    pub fn start(&self, name: &str) -> Result<Arc<Runtime>, String> {
        {
            let rt = self.shared.mu.lock().unwrap();
            if let Some(rt) = rt.get(name) {
                return Ok(rt.clone());
            }
        }
        if !self.store.is_installed(name) {
            return Err(format!("插件未安装: {}", name));
        }
        let plugin_dir = PathBuf::from(&self.home).join("plugins").join(name);
        let mf = load_manifest(&plugin_dir)?;

        let port = {
            let rt = self.shared.mu.lock().unwrap();
            self.alloc_port_locked(&rt, &mf.bind)?
        };

        let cmd_path = plugin_dir.join(&mf.command);
        if !cmd_path.exists() {
            return Err(format!("插件入口不存在: {}", mf.command));
        }

        let log_path = PathBuf::from(&self.home).join("logs").join("plugins").join(format!("{}.log", name));
        if let Some(p) = log_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        rotate_log(&log_path, MAX_LOG_BYTES);
        let log_file = std::fs::OpenOptions::new()
            .create(true).append(true).open(&log_path).map_err(|e| e.to_string())?;

        let mut cmd = std::process::Command::new(&cmd_path);
        cmd.args(&mf.args);
        cmd.current_dir(&plugin_dir);
        cmd.env("PLUGIN_PORT", port.to_string());
        cmd.env("PLUGIN_BIND", &mf.bind.clone());
        cmd.env("PLUGIN_NAME", name);
        cmd.env("PANEL_HOME", &self.home);
        cmd.env("IOTAPANEL_VERSION", crate::config::VERSION);
        let log_capture = log_file.try_clone().map_err(|e| e.to_string())?;
        cmd.stdout(std::process::Stdio::from(log_file));
        cmd.stderr(std::process::Stdio::from(log_capture));
        cmd.stdin(std::process::Stdio::null());

        let child = cmd.spawn().map_err(|e| format!("启动插件进程失败: {}", e))?;
        let mut child = child;
        let pid = child.id() as i32;
        let started_at = chrono::Utc::now().to_rfc3339();

        let rt_arc = Arc::new(Runtime {
            name: name.to_string(),
            port,
            pid,
            bind: mf.bind.clone(),
            start_tick: process::proc_start_tick(pid).unwrap_or(0),
            started_at: started_at.clone(),
            last_use: AtomicU64::new(now_nanos()),
            stop_idle: Arc::new(AtomicBool::new(false)),
        });

        log_header(&log_path, name, port, pid, &started_at);

        {
            let mut rt = self.shared.mu.lock().unwrap();
            rt.insert(name.to_string(), rt_arc.clone());
            self.shared.save_port_map(&rt);
        }

        if process::wait_port(&mf.bind, port, READINESS_TIMEOUT).is_err() {
            {
                let mut rt = self.shared.mu.lock().unwrap();
                if let Some(cur) = rt.get(name) {
                    if Arc::ptr_eq(cur, &rt_arc) {
                        rt.remove(name);
                        self.shared.save_port_map(&rt);
                    }
                }
            }
            process::kill_proc(&rt_arc);
            let _ = child.wait(); // reap
            let tail = tail_log(&log_path, 20);
            return Err(format!("插件启动超时 ({}s)：{}", READINESS_TIMEOUT.as_secs(), tail.trim()));
        }

        self.spawn_reaper(name.to_string(), rt_arc.clone(), child);

        if !self.store.is_keepalive(name) {
            self.arm_idle(rt_arc.clone());
        }
        crate::log_info(&format!("plugin started: {} port={} pid={}", name, port, pid));
        Ok(rt_arc)
    }

    /// Reap the child process and clean up its runtime entry when it exits.
    fn spawn_reaper(&self, name: String, rt: Arc<Runtime>, mut child: std::process::Child) {
        let shared = self.shared.clone();
        let pid = rt.pid;
        std::thread::spawn(move || {
            let _ = child.wait(); // reap zombie
            let (found, _) = {
                let mut map = shared.mu.lock().unwrap();
                let found = match map.get(&name) {
                    Some(cur) => Arc::ptr_eq(cur, &rt),
                    None => false,
                };
                if found {
                    map.remove(&name);
                    shared.save_port_map(&map);
                }
                (found, ())
            };
            if found {
                crate::log_info(&format!("plugin process exited, entry cleaned: {} pid={}", name, pid));
            }
        });
    }

    pub fn stop(&self, name: &str) -> Result<(), String> {
        let rt = {
            let mut map = self.shared.mu.lock().unwrap();
            let rt = map.remove(name).ok_or_else(|| format!("插件未在运行: {}", name))?;
            self.shared.save_port_map(&map);
            rt
        };
        rt.stop_idle.store(true, Ordering::SeqCst);
        process::kill_proc(&rt);
        crate::log_info(&format!("plugin stopped: {} pid={}", name, rt.pid));
        Ok(())
    }

    pub fn restart(&self, name: &str) -> Result<(), String> {
        let _ = self.stop(name);
        self.start(name)?;
        Ok(())
    }

    /// Remove a plugin: stop it if running, then delete its install directory.
    pub fn uninstall(&self, name: &str) -> Result<(), String> {
        let _ = self.stop(name);
        let dir = PathBuf::from(&self.home).join("plugins").join(name);
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
    }

    /// The plugin's configured bind address (manifest `bind`, default 127.0.0.1).
    pub fn bind_of(&self, name: &str) -> String {
        match load_manifest(&PathBuf::from(&self.home).join("plugins").join(name)) {
            Ok(mf) if !mf.bind.is_empty() => mf.bind.clone(),
            _ => "127.0.0.1".to_string(),
        }
    }

    /// Record activity (event-driven idle extension).
    pub fn touch(&self, name: &str) {
        if let Some(rt) = self.shared.mu.lock().unwrap().get(name).cloned() {
            rt.mark_used();
        }
    }

    pub fn status(&self, name: &str) -> Status {
        match self.shared.mu.lock().unwrap().get(name).cloned() {
            Some(rt) => Status { running: true, port: rt.port, pid: rt.pid, started_at: rt.started_at.clone() },
            None => Status { running: false, port: 0, pid: 0, started_at: String::new() },
        }
    }

    pub fn apply_keepalive(&self, name: &str, enabled: bool) {
        let Some(rt) = self.shared.mu.lock().unwrap().get(name).cloned() else { return };
        if enabled {
            rt.stop_idle.store(true, Ordering::SeqCst);
        } else {
            rt.stop_idle.store(false, Ordering::SeqCst);
            if !self.store.is_keepalive(name) {
                self.arm_idle(rt);
            }
        }
    }

    /// Stop all non-keepalive plugins (keepalive processes survive restart).
    pub fn shutdown(&self) {
        let victims: Vec<Arc<Runtime>> = {
            let map = self.shared.mu.lock().unwrap();
            map.iter().filter(|(n, _)| !self.store.is_keepalive(n)).map(|(_, r)| r.clone()).collect()
        };
        for rt in victims {
            rt.stop_idle.store(true, Ordering::SeqCst);
            process::kill_proc(&rt);
        }
    }

    // ---------- internals ----------

    fn arm_idle(&self, rt: Arc<Runtime>) {
        let idle = *self.idle.lock().unwrap();
        if idle <= Duration::ZERO {
            return;
        }
        let stop_flag = rt.stop_idle.clone();
        let store = self.store.clone();
        let shared = self.shared.clone();
        let pid = rt.pid;
        let start_tick = rt.start_tick;
        let name = rt.name.clone();
        std::thread::spawn(move || {
            loop {
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                let idle_elapsed = now_nanos().saturating_sub(rt.last_use.load(Ordering::SeqCst))
                    >= idle.as_nanos() as u64;
                if idle_elapsed {
                    let removed = {
                        let mut map = shared.mu.lock().unwrap();
                        let is_keepalive = store.is_keepalive(&name);
                        let is_same = match map.get(&name) {
                            Some(cur) => Arc::ptr_eq(cur, &rt),
                            None => false,
                        };
                        if is_keepalive || !is_same {
                            false
                        } else {
                            map.remove(&name);
                            shared.save_port_map(&map);
                            true
                        }
                    };
                    if removed {
                        process::kill_proc_helper(pid, start_tick);
                        crate::log_info(&format!("plugin idle-exited, memory released: {}", name));
                    }
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    fn alloc_port_locked(&self, rt: &HashMap<String, Arc<Runtime>>, bind: &str) -> Result<i32, String> {
        let b = if bind.is_empty() { "127.0.0.1" } else { bind };
        for p in self.port_lo..=self.port_hi {
            let in_use = rt.values().any(|r| r.port == p);
            if in_use || is_listening("127.0.0.1", p) || (b != "127.0.0.1" && is_listening(b, p)) {
                continue;
            }
            return Ok(p);
        }
        Err("插件端口池已耗尽".into())
    }

    fn read_port_map(&self) -> HashMap<String, PortMapEntry> {
        match std::fs::read_to_string(&self.shared.port_map_path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }
}

fn now_nanos() -> u64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or(Duration::ZERO);
    (d.as_secs() as u64) * 1_000_000_000 + d.subsec_nanos() as u64
}

fn is_listening(host: &str, port: i32) -> bool {
    use std::net::TcpStream;
    let addr: std::net::SocketAddr = format!("{}:{}", host, port).parse().ok().unwrap_or_else(|| {
        // IPv6 literal support via lookup on "::1" style is rare here.
        "127.0.0.1:0".parse().unwrap()
    });
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

fn rotate_log(path: &Path, max_bytes: u64) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() > max_bytes {
            let _ = std::fs::rename(path, path.with_extension("log.1"));
        }
    }
}

fn log_header(path: &Path, name: &str, port: i32, pid: i32, ts: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o644).open(path) {
        let _ = writeln!(f, "\n=== [{}] start, port={}, pid={}, {} ===", name, port, pid, ts);
    }
}

fn tail_log(path: &Path, n: usize) -> String {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let mut lines: Vec<&str> = data.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() > n {
        lines.drain(..lines.len() - n);
    }
    lines.join("\n")
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}