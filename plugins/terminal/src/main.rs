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

//! 网页终端插件（纯 Rust 重写）。
//!
//! 协议：浏览器 WS ↔ 插件（WS 帧）↔ PTY 伪终端 ↔ shell。
//!   - 浏览器发 Text/Binary/JSON(resize) → 写 PTY / 调整尺寸
//!   - PTY 输出 → WS Binary 帧回浏览器
//!
//! 经面板网关 /p/terminal/ws 访问（面板把浏览器字节原样透传给本插件）。

use iotapanel_sdk::http::{read_request, write_response, Request, Response, DEFAULT_MAX_BODY};
use iotapanel_sdk::ws::{self, Opcode, WsStream};
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::RawFd;
use std::sync::Arc;

const INDEX_HTML: &str = include_str!("../web/index.html");

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(19005);

    let listener = match TcpListener::bind((bind.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[terminal] listen failed: {}", e);
            std::process::exit(1);
        }
    };
    eprintln!("[terminal] listening on {}:{}", bind, port);

    let idx = Arc::new(INDEX_HTML.to_string());
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let idx = idx.clone();
        std::thread::spawn(move || {
            let _ = handle_conn(stream, &idx);
        });
    }
}

fn handle_conn(stream: TcpStream, idx: &str) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(3600))).ok();
    let mut reader = BufReader::new(stream.try_clone()?);
    match read_request(&mut reader, DEFAULT_MAX_BODY) {
        Ok(Some(req)) => {
            let is_ws = req
                .header("upgrade")
                .map(|v| v.eq_ignore_ascii_case("websocket"))
                .unwrap_or(false);
            if req.path.starts_with("/ws") && is_ws {
                drop(reader);
                let ws = upgrade(&stream, &req)?;
                run_shell(stream, ws);
                return Ok(());
            }
            let resp = http_handler(&req, idx);
            let close = !req
                .header("connection")
                .map(|v| v.to_ascii_lowercase().contains("keep-alive"))
                .unwrap_or(false);
            let mut writer = stream.try_clone()?;
            write_response(&mut writer, &resp, req.method == "HEAD", close)?;
        }
        _ => {}
    }
    Ok(())
}

fn upgrade(stream: &TcpStream, req: &Request) -> Result<WsStream, std::io::Error> {
    ws::handshake(&mut stream.try_clone()?, req)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok(WsStream::new(stream.try_clone()?))
}

fn http_handler(req: &Request, idx: &str) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET" | "HEAD", "/api/health") => Response::json(&serde_json::json!({"ok": true})),
        ("GET" | "HEAD", _) => {
            let mut r = Response::html(idx);
            r.headers.push(("Cache-Control".into(), "no-cache".into()));
            r
        }
        _ => Response::json_err(404, "not found"),
    }
}

// ============================== PTY ==============================

struct Pty {
    master: RawFd,
}

impl Pty {
    fn open() -> std::io::Result<Pty> {
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            if master < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::grantpt(master) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::unlockpt(master) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Pty { master })
        }
    }

    fn slave_name(&self) -> std::io::Result<String> {
        unsafe {
            let mut buf = [0u8; 128];
            if libc::ptsname_r(self.master, buf.as_mut_ptr() as *mut libc::c_char, buf.len()) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Ok(String::from_utf8_lossy(&buf[..len]).into_owned())
        }
    }

    fn set_winsize(&self, rows: u16, cols: u16) {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(self.master, libc::TIOCSWINSZ as _, &ws);
        }
    }

    /// 阻塞读 PTY；返回 0 表示 shell 退出（EIO）。
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        unsafe {
            let n = libc::read(self.master, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EIO) {
                    return Ok(0);
                }
                return Err(e);
            }
            Ok(n as usize)
        }
    }

    fn write(&self, data: &[u8]) -> std::io::Result<usize> {
        unsafe {
            let n = libc::write(self.master, data.as_ptr() as *const libc::c_void, data.len());
            if n < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(n as usize)
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe { libc::close(self.master); }
    }
}

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// fork 子进程：创建会话、控制终端，重定向到 slave，exec shell。
fn spawn_shell(slave_name: &str) -> std::io::Result<()> {
    use std::ffi::CString;
    let slave_c = CString::new(slave_name).expect("nul in slave name");
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let shell_c =
        CString::new(shell).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "SHELL 含空字节"))?;
    let argv0 = shell_c.clone();

    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if pid == 0 {
            libc::setsid();
            let slave = libc::open(slave_c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if slave < 0 {
                libc::_exit(1);
            }
            libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
            // 清理会话令牌等敏感环境变量
            libc::unsetenv(b"mp_session\0".as_ptr() as *const libc::c_char);
            libc::unsetenv(b"PLUGIN_TOKEN\0".as_ptr() as *const libc::c_char);

            let args = [argv0.as_ptr(), std::ptr::null()];
            libc::execvp(shell_c.as_ptr(), args.as_ptr());
            libc::_exit(127);
        }
    }
    Ok(())
}

/// 服务端 → 客户端 WS 帧（Server→Client 不掩码）。
fn opcode_u8(op: Opcode) -> u8 {
    match op {
        Opcode::Continuation => 0x0,
        Opcode::Text => 0x1,
        Opcode::Binary => 0x2,
        Opcode::Close => 0x8,
        Opcode::Ping => 0x9,
        Opcode::Pong => 0xA,
        Opcode::Other(v) => v & 0xF,
    }
}

fn write_ws_frame(w: &mut TcpStream, opcode: Opcode, payload: &[u8]) -> std::io::Result<()> {
    let mut hdr = vec![0x80 | opcode_u8(opcode)];
    let len = payload.len();
    if len < 126 {
        hdr.push(len as u8);
    } else if len <= 0xFFFF {
        hdr.push(126);
        hdr.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        hdr.push(127);
        hdr.extend_from_slice(&(len as u64).to_be_bytes());
    }
    w.write_all(&hdr)?;
    w.write_all(payload)
}

fn run_shell(_client: TcpStream, ws: WsStream) {
    let fail = |ws: WsStream, msg: &str| {
        let mut w = ws;
        let _ = write_ws_frame(&mut w.writer, Opcode::Text, msg.as_bytes());
    };

    let pty = match Pty::open() {
        Ok(p) => Arc::new(p),
        Err(e) => return fail(ws, &format!("启动 shell 失败: {}\n", e)),
    };
    pty.set_winsize(DEFAULT_ROWS, DEFAULT_COLS);
    let slave = match pty.slave_name() {
        Ok(s) => s,
        Err(e) => return fail(ws, &format!("启动 shell 失败: {}\n", e)),
    };
    if let Err(e) = spawn_shell(&slave) {
        return fail(ws, &format!("启动 shell 失败: {}\n", e));
    }

    let mut out_writer = match ws.writer.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };

    // 输入线程：读 WS 帧 → 写 PTY（resize 单独处理）
    let pty_in = pty.clone();
    let mut ws_in = ws;
    let input = std::thread::spawn(move || {
        loop {
            match ws_in.read_frame() {
                Ok((op, data)) => match op {
                    Opcode::Close => break,
                    Opcode::Ping => {
                        let _ = ws_in.write_frame(Opcode::Pong, &data);
                        continue;
                    }
                    Opcode::Pong => continue,
                    _ => {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                if v["type"].as_str() == Some("resize") {
                                    let cols = v["cols"].as_u64().unwrap_or(DEFAULT_COLS as u64).clamp(1, 500) as u16;
                                    let rows = v["rows"].as_u64().unwrap_or(DEFAULT_ROWS as u64).clamp(1, 500) as u16;
                                    pty_in.set_winsize(rows, cols);
                                    continue;
                                }
                            }
                        }
                        if pty_in.write(&data).is_err() {
                            break;
                        }
                    }
                },
                Err(_) => break,
            }
        }
        pty_in
    });

    // 输出线程：读 PTY → WS Binary
    let mut buf = [0u8; 4096];
    loop {
        match pty.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if write_ws_frame(&mut out_writer, Opcode::Binary, &buf[..n]).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = write_ws_frame(&mut out_writer, Opcode::Close, &[]);
    let _ = out_writer.shutdown(std::net::Shutdown::Both);
    let _ = input.join();
}
