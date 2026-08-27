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

//! 运行时配置：环境变量 -> PANEL_HOME/etc/.env（环境变量优先）。
//! PANEL_HOME 确定顺序：env > .env > 可执行文件位置推导 > /data/panel。

use iotapanel_sdk::util::{parse_env_file, set_env_var};
use std::path::{Path, PathBuf};

pub const VERSION: &str = "0.4.0";

#[derive(Clone)]
pub struct Config {
    pub home: String,
    pub listen_addr: String,
    pub jwt_secret: String,
    /// 插件空闲退出秒数。
    pub idle_timeout_secs: u64,
    pub port_lo: u16,
    pub port_hi: u16,
    pub trust_proxy: bool,
}

impl Config {
    pub fn load() -> Result<Config, String> {
        let mut home = std::env::var("PANEL_HOME").unwrap_or_default();
        if home.is_empty() {
            home = derive_home_from_exe().unwrap_or_default();
        }
        // 先加载 .env（不覆盖已有环境变量）
        let env_path = Path::new(&home).join("etc").join(".env");
        load_env_file_no_override(&env_path);

        if let Ok(v) = std::env::var("PANEL_HOME") {
            if !v.is_empty() {
                home = v;
            }
        }
        if home.is_empty() {
            home = "/data/panel".to_string();
        }
        // .env 里可能写有 PANEL_HOME，重新定位后需要再读一次对应文件
        let env_path = Path::new(&home).join("etc").join(".env");
        load_env_file_no_override(&env_path);

        let idle_secs = env_parse("IDLE_TIMEOUT", 300u64);
        let cfg = Config {
            home: home.clone(),
            listen_addr: std::env::var("LISTEN_ADDR").unwrap_or_else(|_| ":8787".into()),
            jwt_secret: std::env::var("JWT_SECRET").unwrap_or_default(),
            idle_timeout_secs: idle_secs,
            port_lo: env_parse("PORT_START", 19000),
            port_hi: env_parse("PORT_END", 19999),
            trust_proxy: matches!(
                std::env::var("PANEL_TRUST_PROXY").as_deref(),
                Ok("1") | Ok("true") | Ok("TRUE") | Ok("True")
            ),
        };
        Ok(cfg)
    }

    /// JWT_SECRET 缺失时生成并持久化到 etc/.env。
    pub fn ensure_secret(&mut self) -> Result<(), String> {
        if !self.jwt_secret.is_empty() {
            return Ok(());
        }
        let secret = crate::util::rand_hex(32);
        let path = Path::new(&self.home).join("etc").join(".env");
        set_env_var(&path, "JWT_SECRET", &secret)
            .map_err(|e| format!("写入 JWT_SECRET 失败: {}", e))?;
        self.jwt_secret = secret;
        Ok(())
    }

    pub fn jwt_secret_bytes(&self) -> Vec<u8> {
        self.jwt_secret.clone().into_bytes()
    }

    pub fn env_path(&self) -> PathBuf {
        Path::new(&self.home).join("etc").join(".env")
    }

    /// 设置页改监听端口：保留 host 部分，只替换端口。
    pub fn set_listen_port(&self, port: u16) -> Result<(), String> {
        let host = self
            .listen_addr
            .rsplit_once(':')
            .map(|(h, _)| h.to_string())
            .unwrap_or_default();
        let new_addr = format!("{}:{}", host, port);
        set_env_var(&self.env_path(), "LISTEN_ADDR", &new_addr).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn env_parse<T: std::str::FromStr>(key: &str, def: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(def)
}

/// 解析 .env 但不覆盖已存在的进程环境变量（与 Go 版一致）。
fn load_env_file_no_override(path: &Path) {
    for (k, v) in parse_env_file(path) {
        if std::env::var(&k).map(|x| x.is_empty()).unwrap_or(true) {
            std::env::set_var(&k, &v);
        }
    }
}

/// 标准布局 <安装目录>/bin/iotapanel → 安装目录。
fn derive_home_from_exe() -> Option<String> {
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let dir = exe.parent()?; // .../bin
    if dir.file_name()? != "bin" {
        return None;
    }
    let parent = dir.parent()?;
    if parent == Path::new("/") {
        return None;
    }
    Some(parent.to_string_lossy().into_owned())
}
