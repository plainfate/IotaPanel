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

//! 迷你阻塞式 HTTP/1.1 服务器（线程模型）。
//!
//! 面板核心与官方插件共用这套解析器，保证行为一致：
//! - Keep-Alive、Content-Length 与 chunked 请求体解码、`Expect: 100-continue`
//! - 大小上限防滥用：请求头 64KB / 请求体默认 256MB

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

pub const MAX_HEAD_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_BODY: usize = 256 << 20;

/// 一个已解析的 HTTP 请求。
pub struct Request {
    pub method: String,
    /// 原始 target（含 query string）
    pub target: String,
    /// 去掉 query 的路径（未解码）
    pub path: String,
    /// 客户端 IP（不含端口；会话记录用）
    pub peer_ip: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// 按名取头（大小写不敏感）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self) -> HashMap<String, String> {
        parse_query(self.target.split_once('?').map(|x| x.1).unwrap_or(""))
    }
}

/// HTTP 响应。
#[derive(Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Self { status, headers: Vec::new(), body: Vec::new() }
    }

    pub fn json(v: &serde_json::Value) -> Self {
        Self::json_status(200, v)
    }

    pub fn json_status(status: u16, v: &serde_json::Value) -> Self {
        Self::new(status)
            .header("Content-Type", "application/json; charset=utf-8")
            .with_body(v.to_string().into_bytes())
    }

    pub fn json_err(status: u16, msg: &str) -> Self {
        Self::json_status(status, &serde_json::json!({ "error": msg }))
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .with_body(body.into())
    }

    pub fn text(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200)
            .header("Content-Type", "text/plain; charset=utf-8")
            .with_body(body.into())
    }

    pub fn status(mut self, s: u16) -> Self {
        self.status = s;
        self
    }

    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }

    pub fn with_body(mut self, b: Vec<u8>) -> Self {
        self.body = b;
        self
    }
}

pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        423 => "Locked",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

/// 写出一个完整响应。
pub fn write_response(
    stream: &mut TcpStream,
    resp: &Response,
    head_only: bool,
    close: bool,
) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(256 + resp.body.len().min(64 * 1024));
    out.extend_from_slice(
        format!("HTTP/1.1 {} {}\r\n", resp.status, status_text(resp.status)).as_bytes(),
    );
    let mut has_ct = false;
    for (k, v) in &resp.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_ct = true;
        }
        out.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    if !has_ct && !resp.body.is_empty() {
        out.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
    }
    out.extend_from_slice(b"Server: IotaPanel\r\n");
    out.extend_from_slice(if close { b"Connection: close\r\n" } else { b"Connection: keep-alive\r\n" });
    if head_only {
        out.extend_from_slice(b"Content-Length: 0\r\n\r\n");
    } else {
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", resp.body.len()).as_bytes());
        out.extend_from_slice(&resp.body);
    }
    stream.write_all(&out)?;
    stream.flush()
}

fn read_line_limited(reader: &mut BufReader<TcpStream>, cap: usize) -> std::io::Result<String> {
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
                if buf.len() > cap {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 从流中读取并解析一个请求。返回 `Ok(None)` 表示对端干净关闭。
pub fn read_request(
    reader: &mut BufReader<TcpStream>,
    max_body: usize,
) -> std::io::Result<Option<Request>> {
    let line = read_line_limited(reader, MAX_HEAD_BYTES)?;
    if line.trim().is_empty() {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let target = parts.next().unwrap_or("/").to_string();
    if method.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad request line"));
    }
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();

    let mut headers = Vec::new();
    loop {
        let hl = read_line_limited(reader, MAX_HEAD_BYTES)?;
        let t = hl.trim_end_matches(['\r', '\n']);
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let peer_ip = reader
        .get_ref()
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let req = Request { method, target, path, peer_ip, headers, body: Vec::new() };

    // Expect: 100-continue —— 先应答再收体（curl 大文件上传需要）
    if let Some(exp) = req.header("expect") {
        if exp.eq_ignore_ascii_case("100-continue") {
            reader.get_ref().write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
    }

    let mut body = Vec::new();
    let te = req.header("transfer-encoding").unwrap_or("").to_ascii_lowercase();
    if te.contains("chunked") {
        body = read_chunked_body(reader, max_body)?;
    } else if let Some(cl) = req.header("content-length") {
        let len: usize = cl
            .trim()
            .parse()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad content-length"))?;
        if len > max_body {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too large"));
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        body = buf;
    }

    Ok(Some(Request { body, ..req }))
}

/// 解码 chunked 编码的请求体。
fn read_chunked_body(reader: &mut BufReader<TcpStream>, max_body: usize) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let sz = read_line_limited(reader, 1024)?;
        let sz = sz.trim();
        let sz = sz.split(';').next().unwrap_or("").trim();
        let n = usize::from_str_radix(sz, 16)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk size"))?;
        if n == 0 {
            // 尾随空行 / trailer 头
            loop {
                let t = read_line_limited(reader, 1024)?;
                if t.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            break;
        }
        if body.len() + n > max_body {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too large"));
        }
        let mut buf = vec![0u8; n];
        reader.read_exact(&mut buf)?;
        body.extend_from_slice(&buf);
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
    }
    Ok(body)
}

/// 启动一个多线程 Keep-Alive HTTP 服务器（handler panic 不影响其他连接）。
pub fn serve<H>(bind: &str, port: u16, handler: H) -> std::io::Result<()>
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let listener = TcpListener::bind((bind, port))?;
    let handler = std::sync::Arc::new(handler);
    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let handler = handler.clone();
        std::thread::spawn(move || {
            let h = |req: &Request| handler(req);
            let _ = handle_connection(stream, &h, DEFAULT_MAX_BODY);
        });
    }
    Ok(())
}

fn handle_connection<F>(stream: TcpStream, handler: &F, max_body: usize) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response,
{
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(75))).ok();
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        match read_request(&mut reader, max_body) {
            Ok(None) => return Ok(()),
            Ok(Some(req)) => {
                let head_only = req.method == "HEAD";
                let close = match req.header("connection") {
                    Some(v) => !v.to_ascii_lowercase().contains("keep-alive"),
                    None => false,
                };
                let resp =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&req)))
                        .unwrap_or_else(|_| Response::json_err(500, "内部错误"));
                write_response(&mut writer, &resp, head_only, close)?;
                if close {
                    return Ok(());
                }
            }
            Err(_) => return Ok(()),
        }
    }
}

/// 百分号解码（`+` 视为空格，query 语义）。
pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 百分号编码（保留 RFC3986 unreserved）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 解析 query string 为键值表。
pub fn parse_query(qs: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in qs.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(urldecode(k), urldecode(v));
    }
    map
}

/// multipart/form-data 解析结果的一个字段。
pub struct MultipartPart {
    pub name: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

/// 解析 multipart/form-data 请求体（file-manager 上传用）。
pub fn parse_multipart(content_type: &str, body: &[u8]) -> Option<Vec<MultipartPart>> {
    let boundary = content_type
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("boundary="))?
        .trim_matches('"')
        .to_string();
    let delim_b = format!("--{}", boundary).into_bytes();
    let mut parts = Vec::new();
    let mut pos = find(body, &delim_b, 0)?;

    loop {
        pos += delim_b.len();
        if body[pos..].starts_with(b"--") {
            break; // 结束分隔符
        }
        pos += 2; // \r\n
        let head_end = find(body, b"\r\n\r\n", pos)? + 4;
        let head = String::from_utf8_lossy(&body[pos..head_end]).to_string();
        let next = find(body, &delim_b, head_end)?;
        let mut data_end = next.saturating_sub(2); // 去掉数据尾部 \r\n
        if data_end < head_end {
            data_end = next;
        }
        let mut name = String::new();
        let mut filename = None;
        for l in head.lines() {
            if l.to_ascii_lowercase().starts_with("content-disposition:") {
                for seg in l.split(';').map(str::trim) {
                    if let Some(v) = seg.strip_prefix("name=") {
                        name = v.trim_matches('"').to_string();
                    }
                    if let Some(v) = seg.strip_prefix("filename=") {
                        filename = Some(v.trim_matches('"').to_string());
                    }
                }
            }
        }
        parts.push(MultipartPart { name, filename, data: body[head_end..data_end].to_vec() });
        pos = next;
        if parts.len() > 512 {
            break; // 字段数防御上限
        }
    }
    Some(parts)
}

/// 在切片中查找子串位置（朴素算法；boundary 都很短，足够快）。
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() || from >= haystack.len() {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}
