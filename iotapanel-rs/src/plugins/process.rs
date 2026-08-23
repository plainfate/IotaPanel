//! Unix process helpers: pid/tick checks and graceful kill.
//! Mirrors `internal/plugins/process_unix.go` plus the blocking helpers.

use crate::plugins::Runtime;
use std::time::{Duration, Instant};

/// Read `/proc/<pid>/stat` starttime (overall field 22 -> index 19 after `)`).
pub fn proc_start_tick(pid: i32) -> Option<u64> {
    let data = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let i = data.rfind(')')?;
    if i + 2 >= data.len() {
        return None;
    }
    let fields: Vec<&str> = data[i + 2..].split_whitespace().collect();
    if fields.len() < 20 {
        return None;
    }
    fields[19].parse().ok()
}

/// Is the process alive? (`kill(pid, 0)`).
pub fn process_alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// Confirm `pid` is still the same process captured at `start_tick`.
pub fn same_process(pid: i32, start_tick: u64) -> bool {
    if start_tick > 0 {
        match proc_start_tick(pid) {
            Some(t) => t == start_tick,
            None => false,
        }
    } else {
        process_alive(pid)
    }
}

/// Kill a plugin process: SIGTERM, wait up to 3s, then SIGKILL.
/// Only signals if the pid still matches the original process (avoids PID reuse).
pub fn kill_proc(rt: &Runtime) {
    kill_helper(rt.pid, rt.start_tick);
}

/// Signal-agnostic kill by pid + start_tick.
pub fn kill_proc_helper(pid: i32, start_tick: u64) {
    kill_helper(pid, start_tick);
}

fn kill_helper(pid: i32, start_tick: u64) {
    if pid <= 0 {
        return;
    }
    if !same_process(pid, start_tick) {
        return; // original process gone / PID reused
    }
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    for _ in 0..30 {
        // 100ms * 30 = up to 3s
        std::thread::sleep(Duration::from_millis(100));
        if !same_process(pid, start_tick) {
            return; // exited
        }
    }
    if same_process(pid, start_tick) {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

/// Poll a TCP port until it accepts a connection or the timeout elapses.
pub fn wait_port(bind: &str, port: i32, timeout: Duration) -> Result<(), String> {
    let addr: std::net::SocketAddr = match format!("{}:{}", bind, port).parse() {
        Ok(a) => a,
        Err(_) if bind == "0.0.0.0" => "127.0.0.1".parse().unwrap(),
        Err(_) => return Err("bad host".into()),
    };
    let deadline = Instant::now() + timeout;
    loop {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("timeout waiting for plugin port".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}