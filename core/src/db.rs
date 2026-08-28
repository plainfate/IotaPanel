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

//! 持久化存储：单一 JSON 文件 data/panel.json（内存维护 + 原子写盘）。
//! 字段与 Go 版完全兼容：users / plugins / sessions / settings。
//!
//! 并发模型：全局 Mutex + 写时克隆，每字节写盘都走 .tmp + rename，
//! 上一份数据保留为 .bak（损坏回滚）。单管理员场景足够。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub password_hash: String,
    #[serde(default)]
    pub salt: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_login_at: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PluginRecord {
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keepalive: bool,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Session {
    pub id: i64,
    pub token_hash: String,
    pub jti: String,
    pub username: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub user_agent: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub api: bool,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct StoreData {
    #[serde(default)]
    users: Vec<User>,
    #[serde(default)]
    plugins: Vec<PluginRecord>,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    settings: std::collections::HashMap<String, String>,
}

pub struct Db {
    path: PathBuf,
    data: Mutex<StoreData>,
}

impl Db {
    /// 打开（或创建）panel.json；主文件损坏时回退 .bak。
    pub fn open(home: &str) -> Result<Db, String> {
        let dir = Path::new(home).join("data");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join("panel.json");

        let mut data = StoreData::default();
        let main = std::fs::read_to_string(&path).ok();
        let bak = std::fs::read_to_string(path.with_extension("json.bak")).ok();
        match (&main, &bak) {
            (Some(m), _) => {
                if let Ok(d) = serde_json::from_str::<StoreData>(&m) {
                    data = d;
                } else if let Some(b) = &bak {
                    data = serde_json::from_str(&b)
                        .map_err(|e| format!("解析 panel.json 失败: {}", e))?;
                }
            }
            (None, Some(b)) => {
                data = serde_json::from_str(&b).unwrap_or_default();
            }
            (None, None) => {}
        }
        let _ = std::fs::remove_file(path.with_extension("json.tmp"));
        Ok(Db { path, data: Mutex::new(data) })
    }

    fn save(&self, data: &StoreData) -> Result<(), String> {
        let json = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
        if self.path.exists() {
            let _ = std::fs::rename(&self.path, self.path.with_extension("json.bak"));
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    // ---------- 用户 ----------

    pub fn has_admin(&self) -> bool {
        !self.data.lock().unwrap().users.is_empty()
    }

    pub fn create_user(&self, mut u: User) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        if d.users.iter().any(|x| x.username == u.username) {
            return Err("用户名已存在".into());
        }
        u.id = d.users.len() as i64 + 1;
        if u.created_at.is_empty() {
            u.created_at = crate::util::rfc3339_now();
        }
        d.users.push(u);
        self.save(&d)
    }

    pub fn get_user(&self, name: &str) -> Option<User> {
        self.data
            .lock()
            .unwrap()
            .users
            .iter()
            .find(|u| u.username == name)
            .cloned()
    }

    pub fn update_password(&self, username: &str, hash: &str, salt: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        match d.users.iter_mut().find(|u| u.username == username) {
            Some(u) => {
                u.password_hash = hash.into();
                u.salt = salt.into();
                self.save(&d)
            }
            None => Err(format!("用户不存在: {}", username)),
        }
    }

    pub fn update_last_login(&self, username: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        if let Some(u) = d.users.iter_mut().find(|u| u.username == username) {
            u.last_login_at = crate::util::rfc3339_now();
        }
        self.save(&d)
    }

    /// 改用户名并同步会话表（保持已登录会话有效）。
    pub fn update_username(&self, old: &str, new: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for u in d.users.iter_mut() {
            if u.username == old {
                u.username = new.into();
            }
        }
        for s in d.sessions.iter_mut() {
            if s.username == old {
                s.username = new.into();
            }
        }
        self.save(&d)
    }

    // ---------- 插件记录 ----------

    pub fn list_plugins(&self) -> Vec<PluginRecord> {
        let mut out = self.data.lock().unwrap().plugins.clone();
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    pub fn get_plugin(&self, name: &str) -> Option<PluginRecord> {
        self.data
            .lock()
            .unwrap()
            .plugins
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    pub fn upsert_plugin(&self, p: PluginRecord) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        match d.plugins.iter_mut().find(|x| x.name == p.name) {
            Some(slot) => *slot = p,
            None => d.plugins.push(p),
        }
        self.save(&d)
    }

    pub fn delete_plugin(&self, name: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        d.plugins.retain(|p| p.name != name);
        self.save(&d)
    }

    pub fn set_keepalive(&self, name: &str, v: bool) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        if let Some(p) = d.plugins.iter_mut().find(|p| p.name == name) {
            p.keepalive = v;
        }
        self.save(&d)
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.get_plugin(name).is_some()
    }

    pub fn is_keepalive(&self, name: &str) -> bool {
        self.get_plugin(name).map(|p| p.keepalive).unwrap_or(false)
    }

    // ---------- 设置 ----------

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.data.lock().unwrap().settings.get(key).cloned()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        d.settings.insert(key.into(), value.into());
        self.save(&d)
    }

    // ---------- 会话 ----------

    pub fn create_session(&self, mut s: Session) -> Result<i64, String> {
        let mut d = self.data.lock().unwrap();
        s.id = d.sessions.len() as i64 + 1;
        let id = s.id;
        if s.created_at.is_empty() {
            s.created_at = crate::util::rfc3339_now();
        }
        d.sessions.push(s);
        self.save(&d)?;
        Ok(id)
    }

    pub fn get_session_by_hash(&self, hash: &str) -> Option<Session> {
        self.data
            .lock()
            .unwrap()
            .sessions
            .iter()
            .find(|s| s.token_hash == hash)
            .cloned()
    }

    pub fn list_sessions(&self, username: &str) -> Vec<Session> {
        let d = self.data.lock().unwrap();
        let mut out: Vec<Session> =
            d.sessions.iter().filter(|s| s.username == username).cloned().collect();
        out.reverse(); // 新会话在前
        out
    }

    pub fn revoke_by_jti(&self, jti: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for s in d.sessions.iter_mut() {
            if s.jti == jti {
                s.revoked = true;
            }
        }
        self.save(&d)
    }

    pub fn revoke_by_token_hash(&self, hash: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for s in d.sessions.iter_mut() {
            if s.token_hash == hash && !s.revoked {
                s.revoked = true;
                break;
            }
        }
        self.save(&d)
    }

    /// 下线除 keep_jti 外的所有非 API 会话；返回下线数量。
    pub fn revoke_other_sessions(&self, username: &str, keep_jti: &str) -> Result<u64, String> {
        let mut d = self.data.lock().unwrap();
        let mut n = 0u64;
        for s in d.sessions.iter_mut() {
            if s.api || s.username != username || s.jti == keep_jti || s.revoked {
                continue;
            }
            s.revoked = true;
            n += 1;
        }
        if n > 0 {
            self.save(&d)?;
        }
        Ok(n)
    }

    pub fn revoke_all_sessions(&self, username: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for s in d.sessions.iter_mut() {
            if s.username == username {
                s.revoked = true;
            }
        }
        self.save(&d)
    }

    /// 会话表超限瘦身（>2000 条时清掉已吊销/过期的记录）。
    pub fn prune_sessions(&self) {
        let now = crate::util::now_secs();
        let mut d = self.data.lock().unwrap();
        if d.sessions.len() > 2000 {
            d.sessions.retain(|s| {
                !(s.revoked || s.expires_at.parse::<i64>().map(|e| e < now).unwrap_or(false))
            });
        }
    }
}
