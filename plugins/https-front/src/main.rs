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

//! HTTPS 网关插件（纯 Rust 重写）。
//!
//! 为面板提供 HTTPS 入口：监听 TLS 端口（默认 443），把解密后的 HTTP
//! 请求反向代理到面板监听地址。证书来源：
//!   - mode=selfsigned：首次运行自动用 rcgen 生成自签证书
//!   - mode=cert：读取已有 PEM 证书 + 私钥
//!   - mode=acme：本环境不支持自动签发（无 ACME 客户端库），回退到 cert/selfsigned，
//!     并在页面提示需自行使用 certbot 等签发。

use iotapanel_sdk::util::{self, Yaml};
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("../web/index.html");

#[derive(Clone)]
struct Cfg {
    panel_addr: String,
    mode: String,      // selfsigned | cert | acme
    cert_file: String, // mode=cert
    key_file: String,
    domain: String, // selfsigned CN
    https_port: u16,
}

impl Cfg {
    fn default() -> Self {
        Cfg {
            panel_addr: "127.0.0.1:8787".into(),
            mode: "selfsigned".into(),
            cert_file: String::new(),
            key_file: String::new(),
            domain: "localhost".into(),
            https_port: 443,
        }
    }
    fn from_yaml(y: &Yaml) -> Self {
        Cfg {
            panel_addr: y.str_or("panel_addr", "127.0.0.1:8787"),
            mode: y.str_or("mode", "selfsigned"),
            cert_file: y.str_or("cert_file", ""),
            key_file: y.str_or("key_file", ""),
            domain: y.str_or("domain", "localhost"),
            https_port: y.str_or("https_port", "443").parse().unwrap_or(443),
        }
    }
    fn to_yaml(&self) -> String {
        format!(
            "panel_addr: {}\nmode: {}\ncert_file: \"{}\"\nkey_file: \"{}\"\ndomain: {}\nhttps_port: {}\n",
            self.panel_addr, self.mode, self.cert_file, self.key_file, self.domain, self.https_port
        )
    }
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("PANEL_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/data/panel"))
}

fn cfg_path() -> std::path::PathBuf {
    let d = home_dir().join("etc").join("https-front");
    let _ = std::fs::create_dir_all(&d);
    d.join("config.yaml")
}

fn load_cfg() -> Cfg {
    let path = cfg_path();
    if !path.exists() {
        let _ = std::fs::write(&path, Cfg::default().to_yaml());
    }
    std::fs::read_to_string(&path)
        .map(|t| Cfg::from_yaml(&util::parse_yaml(&t)))
        .unwrap_or_else(|_| Cfg::default())
}

fn save_cfg(cfg: &Cfg) -> std::io::Result<()> {
    std::fs::write(cfg_path(), cfg.to_yaml())
}

/// 自签证书生成：返回 (cert_pem, key_pem)。rcgen 0.13 用法。
fn generate_selfsigned(domain: &str) -> Result<(String, String), String> {
    let kp = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = rcgen::CertificateParams::new(vec![domain.to_string()])
        .and_then(|p| p.self_signed(&kp))
        .map_err(|e| e.to_string())?;
    Ok((cert.pem().to_string(), kp.serialize_pem()))
}

/// 生成并持久化自签证书。返回 (cert_path, key_path)。
fn ensure_selfsigned(cfg: &Cfg) -> Result<(String, String), String> {
    let d = home_dir().join("etc").join("https-front").join("selfsigned");
    let _ = std::fs::create_dir_all(&d);
    let cert_path = d.join("cert.pem");
    let key_path = d.join("key.pem");
    if cert_path.exists() && key_path.exists() {
        return Ok((cert_path.to_string_lossy().into_owned(), key_path.to_string_lossy().into_owned()));
    }
    let (cert, key) = generate_selfsigned(&cfg.domain)?;
    std::fs::write(&cert_path, &cert).map_err(|e| e.to_string())?;
    std::fs::write(&key_path, &key).map_err(|e| e.to_string())?;
    Ok((cert_path.to_string_lossy().into_owned(), key_path.to_string_lossy().into_owned()))
}

/// 组装 rustls ServerConfig（ring provider）。
fn make_server_config(cfg: &Cfg) -> Result<Arc<rustls::ServerConfig>, String> {
    let (cert_file, key_file) = if cfg.mode == "cert" && !cfg.cert_file.is_empty() && !cfg.key_file.is_empty() {
        (cfg.cert_file.clone(), cfg.key_file.clone())
    } else {
        // selfsigned 或 acme(回退 selfsigned)
        ensure_selfsigned(cfg)?
    };

    let certs = load_certs(&cert_file)?;
    let key = load_key(&key_file)?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut scfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("协议版本: {}", e))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("加载证书: {}", e))?;
    Ok(Arc::new(scfg))
}

fn load_certs(path: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let data = std::fs::read(path).map_err(|e| format!("读证书 {}: {}", path, e))?;
    let certs = rustls_pemfile::certs(&mut std::io::BufReader::new(&data[..]))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("解析证书: {}", e))?;
    Ok(certs)
}

fn load_key(path: &str) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let data = std::fs::read(path).map_err(|e| format!("读私钥 {}: {}", path, e))?;
    rustls_pemfile::private_key(&mut std::io::BufReader::new(&data[..]))
        .map_err(|e| format!("解析私钥: {}", e))?
        .ok_or_else(|| format!("私钥文件无有效密钥: {}", path))
}

// ---------- 反向代理（天然 HTTP）----------

/// 处理一个 TLS 连接：完成握手后读请求、转发到面板、回写响应。
fn handle_tls(mut stream: TcpStream, scfg: Arc<rustls::ServerConfig>, panel_addr: String) {
    let conn = rustls::ServerConnection::new(scfg);
    let mut tls = match conn {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut tls_stream = rustls::Stream::new(&mut tls, &mut stream);
    // 读请求头
    let req_bytes = match read_http_request(&mut tls_stream) {
        Some(b) => b,
        None => return,
    };
    // 转发到面板
    match TcpStream::connect(panel_addr.as_str()) {
        Ok(mut upstream) => {
            upstream.set_read_timeout(Some(Duration::from_secs(60))).ok();
            let _ = upstream.write_all(&req_bytes);
            let mut resp = Vec::new();
            let _ = upstream.read_to_end(&mut resp);
            let _ = tls_stream.write_all(&resp);
            let _ = tls_stream.flush();
        }
        Err(_) => {
            let _ = tls_stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
    let _ = tls_stream.flush();
}

/// 从 TLS 流读一个完整 HTTP 请求（请求行 + 头 + body）。
fn read_http_request(r: &mut dyn Read) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let mut head_end: Option<usize> = None;
    // 逐字节找 \r\n\r\n
    while head_end.is_none() {
        match r.read(&mut tmp) {
            Ok(0) | Err(_) => return None,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                head_end = find_sub(&buf, b"\r\n\r\n");
            }
        }
    }
    let head_end = head_end.unwrap();
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let mut content_len = 0usize;
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    // 已读 body 部分
    let have = buf.len().saturating_sub(head_end + 4);
    while have < content_len {
        match r.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    Some(buf)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------- 配置 Web 页面 + API（走纯 HTTP，local）

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port_env = std::env::var("PLUGIN_PORT").ok()
        .and_then(|p| p.parse::<u16>().ok());
    let cfg = load_cfg();
    let https_port = port_env.unwrap_or(cfg.https_port);

    let scfg = match make_server_config(&cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[https-front] 证书加载失败: {}", e);
            std::process::exit(1);
        }
    };
    let panel_addr = cfg.panel_addr.clone();

    // 监听 TLS
    let abs_listen = format!("{}:{}", if bind.is_empty() { "0.0.0.0" } else { &bind }, https_port);
    let listener = match TcpListener::bind(&abs_listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[https-front] 监听失败 {}: {}", abs_listen, e);
            std::process::exit(1);
        }
    };
    eprintln!(
        "[https-front] HTTPS listening on {} (mode={}, panel_addr={})",
        abs_listen, cfg.mode, panel_addr
    );

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let scfg = scfg.clone();
        let panel_addr = panel_addr.clone();
        std::thread::spawn(move || {
            stream.set_nodelay(true).ok();
            handle_tls(stream, scfg, panel_addr);
        });
    }
}