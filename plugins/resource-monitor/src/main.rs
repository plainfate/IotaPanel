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

//! 资源监控插件（纯 Rust 重写）。
//! 读 /proc 解析 CPU/内存/负载/磁盘/进程/网络。经面板网关 /p/resource-monitor/ 访问。
//! API（字段与 web/index.html 前端契约一致）：
//!   GET /api/status  -> 系统当前快照（同 /api/stats）
//!   GET /api/stats   -> 系统当前快照（前端轮询所调）
//!   GET /api/history?points=N -> 最近 CPU/内存采样序列

use iotapanel_sdk::http::{Request, Response};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

const INDEX_HTML: &str = include_str!("../web/index.html");

struct CpuSampler {
    prev: Option<(u128, u128)>, // (idle, total)
}

struct State {
    hist: VecDeque<(i64, f64, f64)>,      // (ts, cpu%, mem%)
    cpu: CpuSampler,
    proc_prev: HashMap<i32, (u64, u64)>,  // pid -> (jiffies, starttime)
    proc_last_ts_ms: u64,                 // 上次采样时刻（毫秒）
    prev_net: Option<(u64, u64, u64)>,    // (rx_bytes, tx_bytes, ts_ms)
}

fn proc_line(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn meminfo() -> (u64, u64, u64, u64) {
    let mut mt = 0; // MemTotal
    let mut ma = 0; // MemAvailable
    let mut st = 0; // SwapTotal
    let mut sf = 0; // SwapFree
    if let Some(s) = proc_line("/proc/meminfo") {
        for line in s.lines() {
            let mut it = line.split_whitespace();
            let key = it.next().unwrap_or("");
            let kb = it.next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            match key {
                "MemTotal:" => mt = kb,
                "MemAvailable:" => ma = kb,
                "SwapTotal:" => st = kb,
                "SwapFree:" => sf = kb,
                _ => {}
            }
        }
    }
    (mt, ma, st, sf)
}

fn cpu_sampler_next(state: &mut CpuSampler) -> f64 {
    let Some(s) = proc_line("/proc/stat") else { return 0.0 };
    let line = s.lines().next().unwrap_or("");
    let nums: Vec<u64> = line.split_whitespace().filter_map(|w| w.parse().ok()).collect();
    if nums.len() < 4 {
        return 0.0;
    }
    let idle = nums[3] as u128;
    let total: u128 = nums.iter().map(|&x| x as u128).sum();
    let usage = match state.prev {
        Some((p_idle, p_total)) if total > p_total => {
            let d_total = total - p_total;
            let d_idle = idle.saturating_sub(p_idle);
            100.0 * ((d_total - d_idle) as f64) / (d_total as f64)
        }
        _ => 0.0,
    };
    state.prev = Some((idle, total));
    usage.clamp(0.0, 100.0)
}

fn cpu_core_count() -> usize {
    let mut n = 0;
    if let Some(s) = proc_line("/proc/cpuinfo") {
        n = s.matches("processor").count();
    }
    if n == 0 {
        n = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1);
    }
    n
}

fn statvfs_total_free(mount: &str) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let c = CString::new(mount.as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
        if r != 0 {
            return None;
        }
        let bsize = s.f_frsize as u64;
        let total = s.f_blocks as u64 * bsize;
        let free = s.f_bfree as u64 * bsize;
        Some((total.saturating_sub(free), total))
    }
    #[cfg(not(unix))]
    {
        let _ = mount;
        None
    }
}

/// 根磁盘使用情况（前端展示 "磁盘 (/)" 用）。
fn root_disk() -> (u64, u64, f64) {
    if let Some((used, total)) = statvfs_total_free("/") {
        let pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
        (used, total, (pct * 10.0).round() / 10.0)
    } else {
        (0, 0, 0.0)
    }
}

fn disks() -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut mounts: Vec<String> = Vec::new();
    if let Some(s) = proc_line("/proc/mounts") {
        for line in s.lines() {
            let mut it = line.split_whitespace();
            it.next(); // dev
            if let Some(mp) = it.next() {
                if mp == "/"
                    || mp.starts_with("/home")
                    || mp.starts_with("/data")
                    || mp.starts_with("/opt")
                    || mp.starts_with("/var")
                    || mp.starts_with("/root")
                {
                    if !mounts.iter().any(|x| x == mp) {
                        mounts.push(mp.to_string());
                    }
                }
            }
        }
    }
    for mp in mounts {
        if let Some((used, total)) = statvfs_total_free(&mp) {
            let pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
            out.push(serde_json::json!({
                "mount": mp,
                "total": total,
                "used": used,
                "usage_percent": (pct * 10.0).round() / 10.0,
            }));
        }
    }
    out
}

fn hostname() -> String {
    proc_line("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()).unwrap_or_default()
}

fn loadavg() -> (f64, f64, f64) {
    if let Some(s) = proc_line("/proc/loadavg") {
        let t: Vec<&str> = s.split_whitespace().collect();
        if t.len() >= 3 {
            let f = |x: &&str| x.parse::<f64>().unwrap_or(0.0);
            return (f(&t[0]), f(&t[1]), f(&t[2]));
        }
    }
    (0.0, 0.0, 0.0)
}

fn uptime() -> u64 {
    // /proc/uptime 形如 "20851.34 20851.34"（整数部分为秒数）
    proc_line("/proc/uptime")
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .and_then(|x| x.split('.').next().map(str::to_string))
        .and_then(|x| x.parse::<u64>().ok())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 读取并汇总 /proc/net/dev 的非 loopback 接口收发字节。
fn net_bytes() -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    if let Some(s) = proc_line("/proc/net/dev") {
        for line in s.lines() {
            let t = line.trim();
            let Some(idx) = t.find(':') else { continue };
            let iface = t[..idx].trim();
            if iface == "lo" {
                continue;
            }
            let rest: Vec<&str> = t[idx + 1..].split_whitespace().collect();
            if rest.len() >= 9 {
                rx = rx.saturating_add(rest[0].parse::<u64>().unwrap_or(0));
                tx = tx.saturating_add(rest[8].parse::<u64>().unwrap_or(0));
            }
        }
    }
    (rx, tx)
}

fn clk_tck() -> f64 {
    // #[cfg(unix)]
    unsafe { libc::sysconf(libc::_SC_CLK_TCK) as f64 }.max(1.0)
}

/// 从 /proc/PID/cmdline 的 argv[0] 取可执行文件基名（当 comm 是动态加载器时的回退）。
fn proc_cmdline_name(pid: i32) -> Option<String> {
    let s = std::fs::read(format!("/proc/{}/cmdline", pid)).ok()?;
    let argv0 = s.split(|&b| b == 0).next()?;
    if argv0.is_empty() {
        return None;
    }
    let p = std::path::Path::new(std::str::from_utf8(argv0).ok()?);
    let base = p.file_name().map(|f| f.to_string_lossy().into_owned())?;
    Some(base)
}

/// 采集按 CPU 排序的前 N 个进程（含 CPU% 与 RSS 内存）。
fn collect_procs(state: &mut State, now: u64, ticks: f64) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    let mut current: HashMap<i32, (u64, u64)> = HashMap::new();
    let pmap: &HashMap<i32, (u64, u64)> = &state.proc_prev;

    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(pid) = name.parse::<i32>() else { continue };
            let Some(s) = proc_line(&format!("/proc/{}/stat", pid)) else { continue };
            // 进程名：取第一个 '(' 与之后第一个 ')' 之间；剥离 "pid (" 前缀与括号
            let comm = {
                let l = s.find('(').map(|i| i + 1).unwrap_or(0);
                let r = s[l..].find(')').map(|i| l + i).unwrap_or(s.len());
                s[l..r].trim().to_string()
            };
            // /proc/PID/stat 字段在最后一个 ')' 之后（state=第3个，utime=第13个，starttime=第20个）
            let f: Vec<&str> = s
                .rsplit_once(')')
                .map(|(_, rest)| rest.split_whitespace().collect())
                .unwrap_or_default();
            if f.len() < 21 {
                continue;
            }
            let utime: u64 = f[13].parse().unwrap_or(0);
            let stime: u64 = f[14].parse().unwrap_or(0);
            let starttime: u64 = f[20].parse().unwrap_or(0);
            let jiffies = utime + stime;
            current.insert(pid, (jiffies, starttime));

            // CPU%（两次采样差值 / 流逝节拍数）
            let cpu = match pmap.get(&pid) {
                Some((pj, _)) => {
                    let dj = jiffies.saturating_sub(*pj);
                    let dt_ms = now.saturating_sub(state.proc_last_ts_ms).max(1);
                    let dt_ticks = (dt_ms as f64 / 1000.0) * ticks;
                    if dt_ticks > 0.0 {
                        (dj as f64 / dt_ticks) * 100.0
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            };

            let mem_kb: u64 = proc_line(&format!("/proc/{}/status", pid))
                .and_then(|st| {
                    st.lines()
                        .find(|l| l.starts_with("VmRSS:"))
                        .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
                })
                .unwrap_or(0);

            let pname = if comm.is_empty() {
                pid.to_string()
            } else if comm.contains("ld-") || comm.contains("dyno") || comm.contains(".so") {
                // 动态/静态-PIE 加载器 comm 被内核截断为 15 字符（如 ld-musl-x86_64.），
                // 回退到 cmdline 的 argv[0] 取真实可执行名
                proc_cmdline_name(pid).unwrap_or_else(|| comm.clone())
            } else {
                comm
            };
            rows.push(serde_json::json!({
                "pid": pid,
                "name": pname,
                "cpu": (cpu * 10.0).round() / 10.0,
                "mem": mem_kb * 1024,
            }));
        }
    }

    state.proc_prev = current;
    state.proc_last_ts_ms = now;

    rows.sort_by(|a, b| {
        b["cpu"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&a["cpu"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.truncate(15);
    rows
}

fn status_json(state: &mut State) -> serde_json::Value {
    let (mt, ma, st, sf) = meminfo();
    let mem_total = mt * 1024;
    let mem_free = ma * 1024;
    let mem_used = mt.saturating_sub(ma) * 1024;
    let swap_total = st * 1024;
    let swap_used = st.saturating_sub(sf) * 1024;
    let cpu = cpu_sampler_next(&mut state.cpu);
    let cpus = cpu_core_count();
    let (l1, l5, l15) = loadavg();
    let now = now_secs();
    let tick = now_ms();
    let mem_pct = if mem_total > 0 { mem_used as f64 / mem_total as f64 * 100.0 } else { 0.0 };
    let swap_pct = if swap_total > 0 { swap_used as f64 / swap_total as f64 * 100.0 } else { 0.0 };
    let (disk_used, disk_total, disk_pct) = root_disk();

    // 网络速率（B/s）
    let (rx, tx) = net_bytes();
    let (mut rx_rate, mut tx_rate) = (0.0, 0.0);
    if let Some((prx, ptx, pts)) = state.prev_net {
        let dt = tick.saturating_sub(pts).max(1) as f64 / 1000.0;
        if dt > 0.0 {
            rx_rate = rx.saturating_sub(prx) as f64 / dt;
            tx_rate = tx.saturating_sub(ptx) as f64 / dt;
        }
    }
    state.prev_net = Some((rx, tx, tick));

    let processes = collect_procs(state, tick, clk_tck());

    state.hist.push_back((now, cpu, mem_pct));
    while state.hist.len() > 120 {
        state.hist.pop_front();
    }

    // 字段严格匹配 web/index.html 前端契约；同时保留核心/swap/disk 数组便于扩展。
    serde_json::json!({
        "hostname": hostname(),
        "os": "",
        "cpu_percent": (cpu * 10.0).round() / 10.0,
        "cpu": { "usage_percent": (cpu * 10.0).round() / 10.0, "cores": cpus },
        "mem": {
            "total": mem_total, "used": mem_used,
            "free": mem_free, "available": mem_free,
            "percent": (mem_pct * 10.0).round() / 10.0,
            "usage_percent": (mem_pct * 10.0).round() / 10.0,
        },
        "swap": {
            "total": swap_total, "used": swap_used,
            "percent": (swap_pct * 10.0).round() / 10.0,
            "usage_percent": (swap_pct * 10.0).round() / 10.0,
        },
        "load": [ (l1 * 100.0).round() / 100.0, (l5 * 100.0).round() / 100.0, (l15 * 100.0).round() / 100.0 ],
        "uptime": uptime(),
        "uptime_seconds": uptime(),
        "disk": { "used": disk_used, "total": disk_total, "percent": disk_pct },
        "disks": disks(),
        "processes": processes,
        "network": {
            "rx_rate": (rx_rate * 10.0).round() / 10.0,
            "tx_rate": (tx_rate * 10.0).round() / 10.0,
            "total_rx": rx,
            "total_tx": tx,
        },
        "timestamp": now,
    })
}

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(19004);
    let state = Arc::new(Mutex::new(State {
        hist: VecDeque::new(),
        cpu: CpuSampler { prev: None },
        proc_prev: HashMap::new(),
        proc_last_ts_ms: 0,
        prev_net: None,
    }));

    let handler = {
        let state = state.clone();
        move |req: &Request| {
            if req.method != "GET" {
                return Response::json_err(405, "method not allowed");
            }
            match req.path.as_str() {
                // 前端轮询 api/stats；/api/status 为后端规范名，两者等价
                "/api/status" | "/api/stats" => {
                    let mut st = state.lock().unwrap();
                    Response::json(&status_json(&mut st))
                }
                "/api/history" => {
                    let q = req.query();
                    let points = q.get("points").and_then(|p| p.parse::<usize>().ok()).unwrap_or(60).min(200);
                    let st = state.lock().unwrap();
                    let hist: Vec<serde_json::Value> = st
                        .hist
                        .iter()
                        .rev()
                        .take(points)
                        .map(|(t, c, m)| serde_json::json!({"t": t, "cpu": c, "mem": m}))
                        .collect();
                    Response::json(&serde_json::json!({"history": hist}))
                }
                _ => {
                    let mut r = Response::html(INDEX_HTML);
                    r.headers.push(("Cache-Control".into(), "no-cache".into()));
                    r
                }
            }
        }
    };

    eprintln!("[resource-monitor] listening on {}:{}", bind, port);
    if let Err(e) = iotapanel_sdk::http::serve(&bind, port, handler) {
        eprintln!("[resource-monitor] server error: {}", e);
        std::process::exit(1);
    }
}