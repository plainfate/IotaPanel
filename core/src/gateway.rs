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

//! 反向代理网关：/p/<插件名>/* → 插件端口。
//! 普通 HTTP 请求转发；WebSocket 升级由 main.rs 连接层做字节级桥接。
//! 注入头：X-Forwarded-Proto / X-Forwarded-Host / X-Panel-Plugin。

use crate::manager::Manager;
use iotapanel_sdk::http::{Request, Response};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

pub struct Gateway {
    pub manager: Arc<Manager>,
    pub trust_proxy: bool,
}

impl Gateway {
    /// 转发一个普通 HTTP 请求（冷启动插件 → 连接 → 回写响应）。
    pub fn handle(
        &self,
        req: &Request,
        name: &str,
        plugin_path: &str,
        client_proto: &str,
        orig_host: &str,
    ) -> Response {
        if let Err(e) = self.manager.start(name) {
            return Response::json_err(502, &e);
        }
        self.manager.touch(name);
        let Some((port, bind)) = self.manager.runtime_of(name) else {
            return Response::json_err(502, "插件运行时不可用");
        };
        let addr = format!("{}:{}", normalize_bind(&bind), port);
        match TcpStream::connect(&addr) {
            Ok(mut upstream) => {
                upstream.set_nodelay(true).ok();
                upstream.set_read_timeout(Some(Duration::from_secs(600))).ok();
                proxy_http(req, name, plugin_path, client_proto, orig_host, &mut upstream)
            }
            Err(e) => Response::json_err(502, &format!("插件连接失败: {}", e)),
        }
    }
}

fn normalize_bind(bind: &str) -> String {
    if bind.contains(':') && !bind.starts_with('[') {
        format!("[{}]", bind)
    } else {
        bind.to_string()
    }
}

pub fn proxy_http(
    req: &Request,
    name: &str,
    plugin_path: &str,
    proto: &str,
    orig_host: &str,
    upstream: &mut TcpStream,
) -> Response {
    let target = match req.target.split_once('?') {
        Some((_, q)) => format!("{}?{}", plugin_path, q),
        None => plugin_path.to_string(),
    };
    let mut head = String::with_capacity(req.headers.len() * 32 + 160);
    head.push_str(&format!("GET {} HTTP/1.1\r\n", target).replace("GET", &req.method));
    let mut has_ua = false;
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("connection")
            || k.eq_ignore_ascii_case("host")
            || k.eq_ignore_ascii_case("content-length")
            || k.eq_ignore_ascii_case("upgrade")
        {
            continue;
        }
        if k.eq_ignore_ascii_case("user-agent") {
            has_ua = true;
        }
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("Host: 127.0.0.1\r\n");
    if !has_ua {
        head.push_str("User-Agent: IotaPanel-Gateway\r\n");
    }
    head.push_str(&format!("X-Forwarded-Proto: {}\r\n", proto));
    head.push_str(&format!("X-Forwarded-Host: {}\r\n", orig_host));
    head.push_str(&format!("X-Panel-Plugin: {}\r\n", name));
    head.push_str("Connection: close\r\n");
    head.push_str(&format!("Content-Length: {}\r\n\r\n", req.body.len()));

    let mut payload = head.into_bytes();
    payload.extend_from_slice(&req.body);
    if upstream.write_all(&payload).is_err() {
        return Response::json_err(502, "插件连接失败: 写入中断");
    }
    let _ = upstream.flush();
    let mut buf = Vec::new();
    // 上游 Connection: close，读到 EOF 即完整响应
    match upstream.read_to_end(&mut buf) {
        Ok(_) if !buf.is_empty() => {}
        Ok(_) => return Response::json_err(502, "插件返回空响应"),
        Err(_) if !buf.is_empty() => {} // 读到部分后中断：尽量解析已有内容
        Err(_) => return Response::json_err(502, "读取插件响应失败"),
    }
    parse_upstream_response(buf).unwrap_or_else(|| Response::json_err(502, "插件响应解析失败"))
}

/// 解析上游插件的 HTTP 响应。
pub fn parse_upstream_response(raw: Vec<u8>) -> Option<Response> {
    let sep = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..sep]).to_string();
    let body = raw[sep + 4..].to_vec();
    let mut lines = head.lines();
    let status_line = lines.next()?;
    let status: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
    let mut resp = Response::new(status);
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            resp.headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    // Content-Length / Connection 由外层统一写
    resp.headers.retain(|(k, _)| {
        !k.eq_ignore_ascii_case("content-length") && !k.eq_ignore_ascii_case("connection")
    });
    resp.body = body;
    Some(resp)
}
