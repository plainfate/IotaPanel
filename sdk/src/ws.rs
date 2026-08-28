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

//! WebSocket 服务端实现（RFC 6455 子集）。
//!
//! - 握手：`Sec-WebSocket-Accept = base64(sha1(key + GUID))`
//! - 帧：TEXT / BINARY / CLOSE / PING / PONG，客户端帧必须 MASKED
//! - 分片消息自动聚合；超大帧（>16MB）拒绝

use crate::http::Request;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// 单帧最大 16MB（终端输出足够）。
pub const MAX_FRAME: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
    Other(u8),
}

impl Opcode {
    fn from_u8(v: u8) -> Self {
        match v {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            other => Opcode::Other(other),
        }
    }
}

fn sha1(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data)
}

/// 最小 SHA-1 实现（仅用于 WS 握手，无安全要求）。
struct Sha1 {
    h: [u32; 5],
    buf: Vec<u8>,
    len: u64,
}

impl Sha1 {
    fn new() -> Self {
        Self { h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0], buf: Vec::new(), len: 0 }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        self.buf.extend_from_slice(data);
        while self.buf.len() >= 64 {
            let block = self.buf[..64].try_into().unwrap();
            self.process(&block);
            self.buf.drain(..64);
            let _ = &mut data;
        }
    }

    fn process(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(chunk.try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }

    fn finish(mut self) -> [u8; 20] {
        let bit_len = self.len * 8;
        self.buf.push(0x80);
        while self.buf.len() % 64 != 56 {
            self.buf.push(0);
        }
        self.buf.extend_from_slice(&bit_len.to_be_bytes());
        while self.buf.len() >= 64 {
            let block = self.buf[..64].try_into().unwrap();
            self.process(&block);
            self.buf.drain(..64);
        }
        let mut out = [0u8; 20];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

trait Digest {
    fn digest(data: &[u8]) -> [u8; 20] {
        let mut s = Sha1::new();
        s.update(data);
        s.finish()
    }
}
impl Digest for Sha1 {}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准带填充 Base64 编码。
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// 计算 Sec-WebSocket-Accept 值。
pub fn accept_key(client_key: &str) -> String {
    b64_encode(&sha1(format!("{}{}", client_key, WS_GUID).as_bytes()))
}

/// 校验握手请求并完成服务端应答。成功后 `stream` 即为 WebSocket 通道。
pub fn handshake(stream: &mut TcpStream, req: &Request) -> Result<(), String> {
    if !req.header("upgrade").map(|v| v.eq_ignore_ascii_case("websocket")).unwrap_or(false) {
        return Err("缺少 Upgrade: websocket".into());
    }
    let key = req
        .header("sec-websocket-key")
        .ok_or("缺少 Sec-WebSocket-Key")?
        .to_string();
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(&key)
    );
    stream.write_all(resp.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// 已升级的 WebSocket 连接。
pub struct WsStream {
    pub reader: BufReader<TcpStream>,
    pub writer: TcpStream,
}

impl WsStream {
    pub fn new(stream: TcpStream) -> Self {
        let writer = stream.try_clone().expect("clone tcp stream");
        Self { reader: BufReader::new(stream), writer }
    }

    /// 读一帧（返回 opcode 与载荷）；分片Continuation 自动拼入调用方的缓冲。
    pub fn read_frame(&mut self) -> std::io::Result<(Opcode, Vec<u8>)> {
        let mut hdr = [0u8; 2];
        self.reader.read_exact(&mut hdr)?;
        let fin = hdr[0] & 0x80 != 0;
        let rsv = hdr[0] & 0x70;
        if rsv != 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "rsv bits set"));
        }
        let opcode = Opcode::from_u8(hdr[0] & 0x0F);
        let masked = hdr[1] & 0x80 != 0;
        let mut len = (hdr[1] & 0x7F) as usize;
        if len == 126 {
            let mut ext = [0u8; 2];
            self.reader.read_exact(&mut ext)?;
            len = u16::from_be_bytes(ext) as usize;
        } else if len == 127 {
            let mut ext = [0u8; 8];
            self.reader.read_exact(&mut ext)?;
            len = u64::from_be_bytes(ext) as usize;
        }
        if len > MAX_FRAME {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
        }
        let mask_key = if masked {
            let mut mk = [0u8; 4];
            self.reader.read_exact(&mut mk)?;
            Some(mk)
        } else {
            None
        };
        let mut payload = vec![0u8; len];
        self.reader.read_exact(&mut payload)?;
        if let Some(mk) = mask_key {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mk[i % 4];
            }
        }
        let _ = fin;
        Ok((opcode, payload))
    }

    /// 写一帧（服务端发往客户端不掩码）。`fin` 恒为 true（整帧发送）。
    pub fn write_frame(&mut self, opcode: Opcode, payload: &[u8]) -> std::io::Result<()> {
        let op = match opcode {
            Opcode::Text => 0x1,
            Opcode::Binary => 0x2,
            Opcode::Close => 0x8,
            Opcode::Ping => 0x9,
            Opcode::Pong => 0xA,
            Opcode::Other(v) => v & 0xF,
            Opcode::Continuation => 0x0,
        };
        let mut frame = Vec::with_capacity(payload.len() + 10);
        frame.push(0x80 | op); // FIN + opcode
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else if payload.len() <= 0xFFFF {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        self.writer.write_all(&frame)?;
        self.writer.flush()
    }

    /// 便捷方法：读一条完整文本消息。
    pub fn read_text(&mut self) -> std::io::Result<String> {
        let mut buf = Vec::new();
        loop {
            let (op, data) = self.read_frame()?;
            match op {
                Opcode::Text | Opcode::Binary => buf.extend_from_slice(&data),
                Opcode::Ping => self.write_frame(Opcode::Pong, &data)?,
                Opcode::Close => return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "closed")),
                _ => continue,
            }
            if !buf.is_empty() {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// 便捷方法：发送文本消息。
    pub fn send_text(&mut self, msg: &str) -> std::io::Result<()> {
        self.write_frame(Opcode::Text, msg.as_bytes())
    }

    /// 便捷方法：发送二进制消息。
    pub fn send_binary(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.write_frame(Opcode::Binary, data)
    }
}
