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

//! 插件进程生命周期：冷启动、端口认领、空闲退出、保活自愈、安装/卸载。
//!
//! 行为对齐 Go 版：
//! - 启动前轮转日志（>20MB 改名 .1），启动后写日志头
//! - 端口就绪等待 6s；失败则 kill 并回读日志尾部 20 行报错
//! - stop 先 SIGTERM，3 秒优雅期后 SIGKILL；发信号前校验 /proc 启动节拍防 PID 复用误杀
//! - 核心退出只杀非保活插件；保活插件跨核心重启复用（port-map.json 认领）

use crate::db::Db;
use crate::manifest::Manifest;
use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const READINESS_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_LOG_BYTES: u64 = 20 << 20;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct PortMapEntry {
    pub port: u16,
    pub pid: i32,
    pub started_at: String,
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub running: bool,
    pub port: u16,
    pub pid: i32,
    pub started_at: String,
}

impl Status {
    fn stopped() -> Self {
        Self { running: false, port: 0, pid: 0, started_at: String::new() }
    }
}

pub struct Runtime {
    pub port: u16,
    pub pid: i32,
    pub bind: String,
    pub start_tick: Option<u64>,
    pub started_at: String,
    pub last_touch: Mutex<Instant>,
}

pub trait PluginStore: Send + Sync {
    fn is_installed(&self, name: &str) -> bool;
    fn is_keepalive(&self, name: &str) -> bool;
}

/// Db 实现 Store。
impl PluginStore for Db {
    fn is_installed(&self, name: &str) -> bool {
        crate::db::Db::is_installed(self, name)
    }
    fn is_keepalive(&self, name: &str) -> bool {
        crate::db::Db::is_keepalive(self, name)
    }
}

/// Arc<Db> 同样满足（管理器持有共享句柄）。
impl<T: PluginStore + ?Sized> PluginStore for std::sync::Arc<T> {
    fn is_installed(&self, name: &str) -> bool {
        (**self).is_installed(name)
    }
    fn is_keepalive(&self, name: &str) -> bool {
        (**self).is_keepalive(name)
    }
}

pub struct Manager {
    pub home: String,
    idle_secs: AtomicU64,
    pub port_lo: u16,
    pub port_hi: u16,
    store: Arc<dyn PluginStore>,
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
    log: SimpleLog,
}

use std::sync::atomic::{AtomicU64, Ordering};

static LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub struct SimpleLog;

impl SimpleLog {
    pub fn info(&self, msg: &str) {
        self.emit("INFO", msg);
    }
    pub fn warn(&self, msg: &str) {
        self.emit("WARN", msg);
    }
    fn emit(&self, level: &str, msg: &str) {
        let line = format!("level={} msg=\"{}\"\n", level, msg);
        let _ = std::io::stderr().write_all(line.as_bytes());
        let lock = LOG_LOCK.get_or_init(|| Mutex::new(()));
        let _g = lock.lock().unwrap();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(Path::new(&LOG_CONTEXT.get_home()).join("logs").join("panel.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// 全局日志上下文（home 需要在 config 加载后写入一次）。
pub static LOG_CONTEXT: LogContext = LogContext { home: OnceLock::new() };

/// 统一日志出口：stderr + logs/panel.log（供全核心调用）。
pub fn log_line(level: &str, msg: &str) {
    let line = format!("level={} msg=\"{}\"\n", level, msg);
    let _ = std::io::stderr().write_all(line.as_bytes());
    let lock = LOG_LOCK.get_or_init(|| Mutex::new(()));
    let _g = lock.lock().unwrap();
    let home = LOG_CONTEXT.get_home();
    if home.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&home).join("logs").join("panel.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

pub struct LogContext {
    home: OnceLock<String>,
}

impl LogContext {
    pub fn set_home(&self, h: &str) {
        let _ = self.home.set(h.to_string());
    }
    pub fn get_home(&self) -> String {
        self.home.get().cloned().unwrap_or_default()
    }
}

impl Manager {
    pub fn new(home: &str, idle_secs: u64, port_lo: u16, port_hi: u16, store: Arc<dyn PluginStore>) -> Arc<Manager> {
        Arc::new(Manager {
            home: home.to_string(),
            idle_secs: AtomicU64::new(idle_secs),
            port_lo,
            port_hi,
            store,
            runtimes: Mutex::new(HashMap::new()),
            log: SimpleLog,
        })
    }

    pub fn set_idle(&self, secs: u64) {
        self.idle_secs.store(secs, Ordering::Relaxed);
    }

    pub fn idle_secs(&self) -> u64 {
        self.idle_secs.load(Ordering::Relaxed)
    }

    fn plugin_dir(&self, name: &str) -> PathBuf {
        Path::new(&self.home).join("plugins").join(name)
    }

    // ---------- 启动 / 停止 ----------

    /// 冷启动插件进程。已在运行则直接返回。
    pub fn start(&self, name: &str) -> Result<Arc<Runtime>, String> {
        {
            let map = self.runtimes.lock().unwrap();
            if let Some(rt) = map.get(name) {
                *rt.last_touch.lock().unwrap() = Instant::now();
                return Ok(rt.clone());
            }
        }
        if !self.store.is_installed(name) {
            return Err(format!("插件未安装: {}", name));
        }
        let dir = self.plugin_dir(name);
        let mf = Manifest::load(&dir)?;
        let bind = if mf.bind.is_empty() { "127.0.0.1".to_string() } else { mf.bind.clone() };
        let port = self.alloc_port(&bind)?;

        let cmd_path = dir.join(&mf.command);
        if !cmd_path.exists() {
            return Err(format!("插件入口不存在: {}", mf.command));
        }

        let log_path = Path::new(&self.home).join("logs").join("plugins").join(format!("{}.log", name));
        if let Some(p) = log_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        rotate_log(&log_path, MAX_LOG_BYTES);
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("打开日志失败: {}", e))?;
        let log_err = log_file.try_clone().map_err(|e| e.to_string())?;

        let mut command = std::process::Command::new(&cmd_path);
        command
            .args(&mf.args)
            .current_dir(&dir)
            .env("PLUGIN_PORT", port.to_string())
            .env("PLUGIN_BIND", &bind)
            .env("PLUGIN_NAME", name)
            .env("PANEL_HOME", &self.home)
            .env("IOTAPANEL_VERSION", crate::config::VERSION)
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(log_err));

        // 记录本进程继承的其余环境变量由 OS 默认传递（与 Go 版 os.Environ 一致）
        let mut child = command.spawn().map_err(|e| format!("启动插件进程失败: {}", e))?;
        let pid = child.id() as i32;

        let header = format!(
            "\n=== [{}] start, port={}, pid={}, {} ===\n",
            name,
            port,
            pid,
            util::rfc3339_now()
        );
        let _ = std::fs::OpenOptions::new().append(true).open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, header.as_bytes()));

        let rt = Arc::new(Runtime {
            port,
            pid,
            bind: bind.clone(),
            start_tick: iotapanel_sdk::util::proc_start_tick(pid),
            started_at: util::rfc3339_now(),
            last_touch: Mutex::new(Instant::now()),
        });
        {
            let mut map = self.runtimes.lock().unwrap();
            map.insert(name.to_string(), rt.clone());
        }
        self.save_port_map();

        // 等端口就绪
        if !wait_port(&bind, port, READINESS_TIMEOUT) {
            self.remove_runtime(name);
            iotapanel_sdk::util::terminate_process(pid);
            let tail = tail_log(&log_path, 20);
            return Err(format!(
                "插件启动超时（{}秒）：{}",
                READINESS_TIMEOUT.as_secs(),
                tail.trim()
            ));
        }

        self.log.info(&format!("plugin started plugin={} port={} pid={}", name, port, pid));
        Ok(rt)
    }

    /// 停止插件并清理端口映射。
    pub fn stop(&self, name: &str) -> Result<(), String> {
        let rt = self.remove_runtime(name).ok_or_else(|| "插件未在运行".to_string())?;
        iotapanel_sdk::util::terminate_process(rt.pid);
        self.log.info(&format!("plugin stopped plugin={} pid={}", name, rt.pid));
        Ok(())
    }

    pub fn restart(&self, name: &str) -> Result<(), String> {
        let _ = self.stop(name);
        self.start(name).map(|_| ())
    }

    fn remove_runtime(&self, name: &str) -> Option<Arc<Runtime>> {
        let rt = self.runtimes.lock().unwrap().remove(name);
        if rt.is_some() {
            self.save_port_map();
        }
        rt
    }

    /// 记录活跃时间（网关转发请求时调用）。
    pub fn touch(&self, name: &str) {
        if let Some(rt) = self.runtimes.lock().unwrap().get(name).cloned() {
            *rt.last_touch.lock().unwrap() = Instant::now();
        }
    }

    pub fn status(&self, name: &str) -> Status {
        match self.runtimes.lock().unwrap().get(name) {
            Some(rt) => Status {
                running: true,
                port: rt.port,
                pid: rt.pid,
                started_at: rt.started_at.clone(),
            },
            None => Status::stopped(),
        }
    }

    pub fn running_count(&self) -> usize {
        self.runtimes.lock().unwrap().len()
    }

    /// 运行中的某个插件运行时（网关用：拿端口+bind）。
    pub fn runtime_of(&self, name: &str) -> Option<(u16, String)> {
        self.runtimes
            .lock()
            .unwrap()
            .get(name)
            .map(|rt| (rt.port, rt.bind.clone()))
    }

    /// ApplyKeepalive：关闭保活时立刻按空闲策略重新武装（空闲扫描循环里生效）。
    pub fn apply_keepalive(&self, name: &str, enabled: bool) {
        if enabled {
            self.log.info(&format!("plugin keepalive enabled plugin={}", name));
        } else {
            self.touch(name);
            self.log.info(&format!("plugin keepalive disabled plugin={}", name));
        }
    }

    // ---------- port-map.json ----------

    fn port_map_path(&self) -> PathBuf {
        Path::new(&self.home).join("etc").join("port-map.json")
    }

    fn save_port_map(&self) {
        let map = self.runtimes.lock().unwrap();
        let mut out = std::collections::BTreeMap::new();
        for (name, rt) in map.iter() {
            out.insert(
                name.clone(),
                PortMapEntry {
                    port: rt.port,
                    pid: rt.pid,
                    started_at: rt.started_at.clone(),
                },
            );
        }
        let data = serde_json::to_string_pretty(&out).unwrap_or_default();
        let path = self.port_map_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, data.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// 核心启动时扫描 port-map.json：端口仍活着直接认领，失效记录清理；
    /// 随后拉起所有保活未运行的插件。
    pub fn load_adopt(&self) {
        {
            let entries: HashMap<String, PortMapEntry> = std::fs::read_to_string(self.port_map_path())
                .ok()
                .and_then(|d| serde_json::from_str(&d).ok())
                .unwrap_or_default();
            let mut map = self.runtimes.lock().unwrap();
            for (name, e) in entries {
                if e.port == 0 || !is_listening_port("127.0.0.1", e.port) {
                    self.log.info(&format!("drop stale port-map entry plugin={}", name));
                    continue;
                }
                let bind = Manifest::load(&self.plugin_dir(&name))
                    .map(|m| if m.bind.is_empty() { "127.0.0.1".into() } else { m.bind })
                    .unwrap_or_else(|_| "127.0.0.1".into());
                map.insert(
                    name.clone(),
                    Arc::new(Runtime {
                        port: e.port,
                        pid: e.pid,
                        bind,
                        start_tick: iotapanel_sdk::util::proc_start_tick(e.pid),
                        started_at: e.started_at,
                        last_touch: Mutex::new(Instant::now()),
                    }),
                );
                self.log.info(&format!("adopted running plugin plugin={} port={} pid={}", name, e.port, e.pid));
            }
        }
        self.revive_keepalive();
    }

    /// 拉起所有「已启用保活但未运行」的插件。
    pub fn revive_keepalive(&self) {
        let Ok(rd) = std::fs::read_dir(self.plugin_dir("")) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() || !self.store.is_keepalive(&name) {
                continue;
            }
            if self.runtimes.lock().unwrap().contains_key(&name) {
                continue;
            }
            if let Err(err) = self.start(&name) {
                self.log.warn(&format!("revive keepalive failed plugin={} err={}", name, err));
            }
        }
    }

    /// 核心 graceful shutdown：仅停非保活插件。
    pub fn shutdown(&self) {
        let victims: Vec<String> = {
            let map = self.runtimes.lock().unwrap();
            map.keys()
                .filter(|n| !self.store.is_keepalive(n))
                .cloned()
                .collect()
        };
        for n in victims {
            let _ = self.stop(&n);
        }
    }

    /// 空闲扫描（后台线程每 5 秒调用一次）：
    /// - 非保活且超过空闲时限 → 杀进程释放内存
    /// - 进程意外死亡 → 清理运行条目（防网关持续 502）
    pub fn sweep_idle(&self) {
        let idle = self.idle_secs();
        let now = Instant::now();
        let mut expired: Vec<(String, i32)> = Vec::new();
        let mut dead: Vec<(String, i32)> = Vec::new();
        {
            let map = self.runtimes.lock().unwrap();
            for (name, rt) in map.iter() {
                let alive = proc_alive_with_tick(rt.pid, rt.start_tick);
                if !alive {
                    dead.push((name.clone(), rt.pid));
                    continue;
                }
                if !self.store.is_keepalive(name)
                    && idle > 0
                    && now.duration_since(*rt.last_touch.lock().unwrap()).as_secs() >= idle
                {
                    expired.push((name.clone(), rt.pid));
                }
            }
        }
        for (name, _) in dead {
            self.remove_runtime(&name);
            self.log.warn(&format!("plugin process exited, entry cleaned plugin={}", name));
        }
        for (name, pid) in expired {
            self.remove_runtime(&name);
            iotapanel_sdk::util::terminate_process(pid);
            self.log.info(&format!("plugin idle-exited, memory released plugin={}", name));
        }
    }

    // ---------- 安装 / 卸载 ----------

    /// 卸载：停止 → 删目录 → 由调用方删除 DB 记录。
    pub fn uninstall(&self, name: &str) -> Result<(), String> {
        let _ = self.stop(name);
        let dir = self.plugin_dir(name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn alloc_port(&self, bind: &str) -> Result<u16, String> {
        let in_use: Vec<u16> =
            self.runtimes.lock().unwrap().values().map(|rt| rt.port).collect();
        for p in self.port_lo..=self.port_hi {
            if in_use.contains(&p) || is_listening_port("127.0.0.1", p) ||
               (bind != "127.0.0.1" && is_listening_port(bind, p)) {
                continue;
            }
            return Ok(p);
        }
        Err("插件端口池已耗尽".into())
    }
}

fn proc_alive_with_tick(pid: i32, tick: Option<u64>) -> bool {
    match tick {
        Some(t0) => iotapanel_sdk::util::proc_start_tick(pid) == Some(t0),
        None => iotapanel_sdk::util::process_alive(pid),
    }
}

pub fn is_listening_port(host: &str, port: u16) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &format!("{}:{}", normalize_host(host), port).parse().expect("addr"),
        Duration::from_millis(300),
    )
    .is_ok()
}

fn normalize_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    }
}

pub fn wait_port(bind: &str, port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_listening_port(bind, port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn rotate_log(path: &Path, max_bytes: u64) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() > max_bytes {
            let _ = std::fs::rename(path, path.with_extension("log.1"));
        }
    }
}

fn tail_log(path: &Path, n: usize) -> String {
    let Ok(data) = std::fs::read_to_string(path) else { return String::new() };
    let lines: Vec<&str> = data.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}
