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

//! 口令哈希（PBKDF2-SHA256）与会话令牌（HMAC-SHA256 签名 cookie）。
//! 存储格式与 Go 版完全兼容：salt 为 "iterations:hex"，旧版纯 hex 按 10 万次；
//! 令牌为 base64url(json{uid,u,exp,j}) + "." + base64url(HMAC-SHA256)。

use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub const COOKIE_NAME: &str = "mp_session";
const ITERATIONS: u32 = 600_000;
const LEGACY_ITERATIONS: u32 = 100_000;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;

type HmacSha256 = Hmac<Sha256>;

/// 生成 salt（"iterations:hex"）与 PBKDF2 哈希。
pub fn hash_password(pw: &str) -> Result<(String, String), String> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| e.to_string())?;
    let mut dk = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha256>(pw.as_bytes(), &salt, ITERATIONS, &mut dk);
    Ok((format!("{}:{}", ITERATIONS, hex::encode(salt)), hex::encode(dk)))
}

/// 解析盐："600000:hex" 新格式或旧版纯 hex（按 10 万次）。
fn parse_salt(salt: &str) -> Option<(u32, Vec<u8>)> {
    if let Some((n, rest)) = salt.split_once(':') {
        if let Ok(iter) = n.parse::<u32>() {
            if iter > 0 {
                if let Ok(b) = hex::decode(rest) {
                    return Some((iter, b));
                }
            }
        }
    }
    hex::decode(salt).ok().map(|b| (LEGACY_ITERATIONS, b))
}

/// 恒定时间口令校验。
pub fn verify_password(pw: &str, salt: &str, hash_hex: &str) -> bool {
    let Some((iter, salt_bytes)) = parse_salt(salt) else { return false };
    if salt_bytes.is_empty() {
        return false;
    }
    let mut dk = [0u8; KEY_LEN];
    pbkdf2_hmac::<Sha256>(pw.as_bytes(), &salt_bytes, iter, &mut dk);
    let ok: bool = hex::encode(dk).as_bytes().ct_eq(hash_hex.as_bytes()).into();
    ok
}

/// 哈希参数与当前不一致时需要升级重哈希。
pub fn needs_rehash(salt: &str) -> bool {
    parse_salt(salt).map(|(iter, _)| iter != ITERATIONS).unwrap_or(false)
}

/// 会话载荷（字段名与 Go 版一致：uid / u / exp / j）。
#[derive(Serialize, Deserialize, Clone)]
pub struct SessionClaims {
    pub uid: i64,
    pub u: String,
    pub exp: i64,
    pub j: String,
}

/// 新会话载荷（含随机 JTI）。ttl 秒有效。
pub fn new_session(uid: i64, username: &str, ttl_secs: i64) -> SessionClaims {
    SessionClaims {
        uid,
        u: username.to_string(),
        exp: crate::util::now_secs() + ttl_secs,
        j: crate::util::rand_hex(16),
    }
}

fn sign(data: &str, secret: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(data.as_bytes());
    b64_url_nopad(&mac.finalize().into_bytes())
}

fn b64_url_nopad(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn b64_decode(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data).ok()
}

/// 签发 "payload.signature" 形式令牌。
pub fn token_for(sess: &SessionClaims, secret: &[u8]) -> String {
    let payload = serde_json::to_string(sess).unwrap_or_default();
    let b64 = b64_url_nopad(payload.as_bytes());
    format!("{}.{}", b64, sign(&b64, secret))
}

/// 校验签名与过期时间。
pub fn parse_token(token: &str, secret: &[u8]) -> Option<SessionClaims> {
    let (payload, sig) = token.split_once('.')?;
    let expect = sign(payload, secret);
    let sig_ok: bool = sig.as_bytes().ct_eq(expect.as_bytes()).into();
    if !sig_ok {
        return None;
    }
    let raw = b64_decode(payload)?;
    let sess: SessionClaims = serde_json::from_slice(&raw).ok()?;
    if sess.exp < crate::util::now_secs() {
        return None;
    }
    Some(sess)
}
