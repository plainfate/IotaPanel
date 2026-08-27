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

//! 核心内部工具：JSON 响应体、哈希、随机数。

use sha2::{Digest, Sha256};

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// UTC 的 RFC3339 字符串。
pub fn rfc3339(secs: i64) -> String {
    iotapanel_sdk::util::rfc3339(secs)
}

pub fn rfc3339_now() -> String {
    iotapanel_sdk::util::rfc3339_now()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn sha256_hex_str(s: &str) -> String {
    sha256_hex(s.as_bytes())
}

/// 密码学安全随机十六进制串（n 字节 → 2n 字符）。
pub fn rand_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).expect("system rng");
    hex::encode(buf)
}

/// JSON 对象辅助构造。
#[macro_export]
macro_rules! jobj {
    ($($k:expr => $v:expr),* $(,)?) => {
        serde_json::json!({ $($k: $v),* })
    };
}
