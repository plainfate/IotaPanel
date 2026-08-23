//! Lightweight JSON file storage (`data/panel.json`). The on-disk schema is
//! byte-compatible with the original Go `internal/db` (users/plugins/sessions/
//! settings), so an existing `panel.json` keeps working unchanged.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct User {
    pub id: i64,
    pub username: String,
    #[serde(rename = "password_hash")]
    pub password_hash: String,
    pub salt: String,
    #[serde(rename = "created_at")]
    pub created_at: String,
    #[serde(rename = "last_login_at")]
    #[serde(default)]
    pub last_login_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PluginRecord {
    pub name: String,
    pub title: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub keepalive: bool,
    #[serde(rename = "installed_at")]
    pub installed_at: String,
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    pub id: i64,
    #[serde(rename = "token_hash")]
    pub token_hash: String,
    pub jti: String,
    pub username: String,
    pub ip: String,
    #[serde(rename = "user_agent")]
    pub user_agent: String,
    #[serde(rename = "created_at")]
    pub created_at: String,
    #[serde(rename = "expires_at")]
    pub expires_at: String,
    pub revoked: bool,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
struct StoreData {
    #[serde(default)]
    users: Vec<User>,
    #[serde(default)]
    plugins: Vec<PluginRecord>,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    settings: BTreeMap<String, String>,
}

pub struct Db {
    path: PathBuf,
    data: Mutex<StoreData>,
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl Db {
    pub fn open(home: &str) -> Result<Db, String> {
        let data_dir = PathBuf::from(home).join("data");
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let path = data_dir.join("panel.json");
        let mut data = StoreData::default();
        let loaded = load_into(&path, &mut data);
        if !loaded {
            // primary missing/corrupt -> fall back to backup
            let bak = path.with_extension("json.bak");
            let _ = load_into(&bak, &mut data);
        }
        if data.settings.is_empty() {
            // keep as empty map, same as Go default
        }
        let _ = std::fs::remove_file(path.with_extension("json.tmp"));
        Ok(Db { path, data: Mutex::new(data) })
    }

    pub fn close(&self) -> Result<(), String> {
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        let data = self.data.lock().unwrap().clone();
        save_store(&self.path, &data)
    }

    // ---------- users ----------

    pub fn has_admin(&self) -> bool {
        !self.data.lock().unwrap().users.is_empty()
    }

    pub fn create_user(&self, mut u: User) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for e in &d.users {
            if e.username == u.username {
                return Err("用户名已存在".into());
            }
        }
        u.id = d.users.len() as i64 + 1;
        if u.created_at.is_empty() {
            u.created_at = now();
        }
        d.users.push(u);
        save_store(&self.path, &d)
    }

    pub fn get_user_by_name(&self, name: &str) -> Result<User, String> {
        let d = self.data.lock().unwrap();
        for u in &d.users {
            if u.username == name {
                return Ok(u.clone());
            }
        }
        Err(format!("用户不存在: {}", name))
    }

    pub fn update_password(&self, username: &str, hash: &str, salt: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for u in d.users.iter_mut() {
            if u.username == username {
                u.password_hash = hash.to_string();
                u.salt = salt.to_string();
                return save_store(&self.path, &d);
            }
        }
        Err(format!("用户不存在: {}", username))
    }

    pub fn update_last_login(&self, username: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for u in d.users.iter_mut() {
            if u.username == username {
                u.last_login_at = now();
                return save_store(&self.path, &d);
            }
        }
        Err(format!("用户不存在: {}", username))
    }

    pub fn update_username(&self, old: &str, new: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for u in d.users.iter_mut() {
            if u.username == old {
                u.username = new.to_string();
            }
        }
        for s in d.sessions.iter_mut() {
            if s.username == old {
                s.username = new.to_string();
            }
        }
        save_store(&self.path, &d)
    }

    // ---------- plugins ----------

    /// Plugins sorted by title (case-sensitive byte order, like Go `<`).
    pub fn list_plugins(&self) -> Vec<PluginRecord> {
        let d = self.data.lock().unwrap();
        let mut out = d.plugins.clone();
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    pub fn get_plugin(&self, name: &str) -> Option<PluginRecord> {
        let d = self.data.lock().unwrap();
        d.plugins.iter().find(|p| p.name == name).cloned()
    }

    pub fn upsert_plugin(&self, mut p: PluginRecord) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for e in d.plugins.iter_mut() {
            if e.name == p.name {
                *e = p;
                return save_store(&self.path, &d);
            }
        }
        if p.installed_at.is_empty() {
            p.installed_at = now();
        }
        d.plugins.push(p);
        save_store(&self.path, &d)
    }

    pub fn delete_plugin(&self, name: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        if let Some(i) = d.plugins.iter().position(|p| p.name == name) {
            d.plugins.remove(i);
            return save_store(&self.path, &d);
        }
        Ok(())
    }

    pub fn set_keepalive(&self, name: &str, v: bool) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for p in d.plugins.iter_mut() {
            if p.name == name {
                p.keepalive = v;
                return save_store(&self.path, &d);
            }
        }
        Ok(())
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.get_plugin(name).is_some()
    }

    pub fn is_keepalive(&self, name: &str) -> bool {
        self.get_plugin(name).map(|p| p.keepalive).unwrap_or(false)
    }

    // ---------- settings ----------

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.data.lock().unwrap().settings.get(key).cloned()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        d.settings.insert(key.to_string(), value.to_string());
        save_store(&self.path, &d)
    }

    // ---------- sessions ----------

    pub fn create_session(&self, mut s: Session) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        s.id = d.sessions.len() as i64 + 1;
        if s.created_at.is_empty() {
            s.created_at = now();
        }
        d.sessions.push(s);
        save_store(&self.path, &d)
    }

    pub fn get_session_by_token_hash(&self, hash: &str) -> Option<Session> {
        let d = self.data.lock().unwrap();
        d.sessions.iter().find(|s| s.token_hash == hash).cloned()
    }

    /// Sessions for a user, most recent first (rev)
    pub fn list_sessions(&self, username: &str) -> Vec<Session> {
        let d = self.data.lock().unwrap();
        let mut out: Vec<Session> = d.sessions.iter().filter(|s| s.username == username).cloned().collect();
        out.reverse();
        out
    }

    pub fn revoke_session_by_jti(&self, jti: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for s in d.sessions.iter_mut() {
            if s.jti == jti {
                s.revoked = true;
                return save_store(&self.path, &d);
            }
        }
        Ok(())
    }

    pub fn revoke_session_by_token_hash(&self, hash: &str) -> Result<(), String> {
        let mut d = self.data.lock().unwrap();
        for s in d.sessions.iter_mut() {
            if s.token_hash == hash && !s.revoked {
                s.revoked = true;
                return save_store(&self.path, &d);
            }
        }
        Ok(())
    }

    /// Revoke all sessions for a user except `keep_jti`; returns count revoked.
    pub fn revoke_other_sessions(&self, username: &str, keep_jti: &str) -> Result<i64, String> {
        let mut d = self.data.lock().unwrap();
        let mut n: i64 = 0;
        for s in d.sessions.iter_mut() {
            if s.username == username && s.jti != keep_jti && !s.revoked {
                s.revoked = true;
                n += 1;
            }
        }
        if n > 0 {
            save_store(&self.path, &d)?;
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
        save_store(&self.path, &d)
    }
}

fn load_into(path: &Path, data: &mut StoreData) -> bool {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<StoreData>(&s) {
            Ok(d) => {
                *data = d;
                true
            }
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Atomic write: current file -> `.bak`, write `.tmp`, rename over.
fn save_store(path: &Path, data: &StoreData) -> Result<(), String> {
    let json = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    if path.exists() {
        let _ = std::fs::rename(path, path.with_extension("json.bak"));
    }
    let tmp = path.with_extension("json.tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::write(&tmp, &json);
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}