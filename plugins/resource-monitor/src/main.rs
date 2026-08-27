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
//! 读 /proc 解析 CPU/内存/负载/磁盘。经面板网关 /p/resource-monitor/ 访问。
//! API:
//!   GET /api/status   -> 系统当前快照
//!   GET /api/history?points=N -> 最近 CPU/内存采样序列

use iotapanel_sdk::http::{Request, Response};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const INDEX_HTML: &str = include_str!("../web/index.html");

struct CpuSampler {
    prev: Option<(u128, u128)>, // (idle, total)
}

struct State {
    hist: VecDeque<(i64, f64, f64)>, // (ts, cpu%, mem%)
    cpu: CpuSampler,
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

fn statvfs_usage(mount: &str) -> Option<(u64, u64)> {
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
        if let Some((used, total)) = statvfs_usage(&mp) {
            let pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
            out.push(serde_json::json!({
                "mount": mp,
                "fstype": "",
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
    proc_line("/proc/uptime")
        .and_then(|s| s.split_whitespace().next().map(|x| x.to_string()))
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}

fn os_name() -> String {
    if let Some(s) = proc_line("/etc/os-release") {
        for line in s.lines() {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                return v.trim_matches('"').to_string();
            }
        }
    }
    proc_line("/proc/sys/kernel/ostype").unwrap_or_else(|| "Linux".into())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    let mem_pct = if mem_total > 0 { mem_used as f64 / mem_total as f64 * 100.0 } else { 0.0 };
    let swap_pct = if swap_total > 0 { swap_used as f64 / swap_total as f64 * 100.0 } else { 0.0 };

    state.hist.push_back((now, cpu, mem_pct));
    while state.hist.len() > 120 {
        state.hist.pop_front();
    }

    serde_json::json!({
        "hostname": hostname(),
        "os": os_name(),
        "load": {
            "1": (l1 * 100.0).round() / 100.0,
            "5": (l5 * 100.0).round() / 100.0,
            "15": (l15 * 100.0).round() / 100.0,
        },
        "cpu": { "usage_percent": (cpu * 10.0).round() / 10.0, "cores": cpus },
        "mem": {
            "total": mem_total, "used": mem_used,
            "free": mem_free, "available": mem_free,
            "usage_percent": (mem_pct * 10.0).round() / 10.0,
        },
        "swap": { "total": swap_total, "used": swap_used, "usage_percent": (swap_pct * 10.0).round() / 10.0 },
        "disk": disks(),
        "uptime_seconds": uptime(),
        "timestamp": now,
    })
}

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(19004);
    let state = Arc::new(Mutex::new(State {
        hist: VecDeque::new(),
        cpu: CpuSampler { prev: None },
    }));

    let handler = {
        let state = state.clone();
        move |req: &Request| {
            if req.method != "GET" {
                return Response::json_err(405, "method not allowed");
            }
            match req.path.as_str() {
                "/api/status" => {
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