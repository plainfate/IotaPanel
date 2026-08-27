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

//! 面板 REST API 与前端页面路由（与 Go 版 API 契约逐条对齐）。

use crate::auth;
use crate::config::Config;
use crate::db::{Db, PluginRecord, Session};
use crate::gateway::Gateway;
use crate::installer;
use crate::manifest::Manifest;
use crate::manager::Manager;
use crate::util;
use iotapanel_sdk::http::{Request, Response};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct LoginGuard {
    inner: Mutex<GuardInner>,
}

#[derive(Default)]
struct GuardInner {
    fails: HashMap<String, u32>,
    until: HashMap<String, Instant>,
}

impl LoginGuard {
    pub fn new() -> Self {
        Self { inner: Mutex::new(GuardInner::default()) }
    }

    fn remaining_secs(&self, user: &str) -> u64 {
        let g = self.inner.lock().unwrap();
        match g.until.get(user) {
            Some(t) if *t > Instant::now() => (*t - Instant::now()).as_secs(),
            _ => 0,
        }
    }

    fn record_fail(&self, user: &str, limit: u32, lock_minutes: u64) -> bool {
        let mut g = self.inner.lock().unwrap();
        let n = g.fails.entry(user.to_string()).or_insert(0);
        *n += 1;
        if *n >= limit {
            g.until.insert(
                user.to_string(),
                Instant::now() + Duration::from_secs(lock_minutes * 60),
            );
            return true;
        }
        false
    }

    fn reset(&self, user: &str) {
        let mut g = self.inner.lock().unwrap();
        g.fails.remove(user);
        g.until.remove(user);
    }
}

#[derive(Default)]
struct SetupProgress {
    running: bool,
    done: usize,
    total: usize,
    current: String,
    complete: bool,
    error: String,
}

impl SetupProgress {
    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "running": self.running,
            "done": self.done,
            "total": self.total,
            "current": self.current,
            "complete": self.complete,
            "error": self.error,
        })
    }
}

pub struct Server {
    pub cfg: Mutex<Config>,
    pub db: Arc<Db>,
    pub manager: Arc<Manager>,
    pub gateway: Gateway,
    guard: LoginGuard,
    prog: Mutex<SetupProgress>,
    start: Instant,
}

impl Server {
    pub fn new(cfg: Config, db: Arc<Db>, manager: Arc<Manager>) -> Arc<Server> {
        let trust_proxy = cfg.trust_proxy;
        Arc::new(Server {
            cfg: Mutex::new(cfg),
            db,
            manager: manager.clone(),
            gateway: Gateway { manager, trust_proxy },
            guard: LoginGuard::new(),
            prog: Mutex::new(SetupProgress::default()),
            start: Instant::now(),
        })
    }

    // ================= 认证辅助 =================

    /// 校验 cookie → 数据库会话记录。失败返回要写出的 Response。
    pub fn session_of(
        &self,
        req: &Request,
    ) -> Result<(auth::SessionClaims, Session), Response> {
        let cookie =
            header_cookie(req, auth::COOKIE_NAME).ok_or_else(|| unauthorized("未登录"))?;
        let claims = auth::parse_token(&cookie, &self.cfg.lock().unwrap().jwt_secret_bytes())
            .ok_or_else(|| unauthorized("会话无效或已过期"))?;
        let rec = self
            .db
            .get_session_by_hash(&util::sha256_hex_str(&cookie))
            .filter(|s| !s.revoked)
            .ok_or_else(|| unauthorized("会话已失效（可能已被强制下线）"))?;
        // 以数据库记录的用户名为准（改用户名后会话仍有效）
        let claims = auth::SessionClaims { u: rec.username.clone(), ..claims };
        Ok((claims, rec))
    }

    pub fn logged_in(&self, req: &Request) -> bool {
        self.session_of(req).is_ok()
    }

    fn csrf_ok(&self, req: &Request) -> bool {
        let Some(origin) = req.header("origin") else { return true };
        if origin.is_empty() {
            return true;
        }
        let host = incoming_host(req, self.cfg.lock().unwrap().trust_proxy);
        match origin.split_once("://") {
            Some((_, h)) => h.eq_ignore_ascii_case(&host),
            None => origin.eq_ignore_ascii_case(&host),
        }
    }

    fn security_policy(&self) -> (u32, u64) {
        let fail_limit = self
            .db
            .get_setting("login_fail_limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let lock_minutes = self
            .db
            .get_setting("login_fail_lock_minutes")
            .and_then(|v| v.parse().ok())
            .unwrap_or(15);
        (fail_limit, lock_minutes)
    }

    // ================= 主路由入口 =================

    /// 处理一个普通请求（WS 升级在 main.rs 连接层提前拦截）。
    pub fn handle(self: &Arc<Self>, req: &Request) -> Response {
        if matches!(req.method.as_str(), "POST" | "PUT" | "DELETE" | "PATCH")
            && !self.csrf_ok(req)
        {
            return with_security_headers(Response::json_err(403, "跨站请求被拒绝"));
        }

        let segs: Vec<&str> = req.path.split('/').filter(|s| !s.is_empty()).collect();
        let method = req.method.as_str();

        let result = match (method, segs.as_slice()) {
            ("GET", ["api", "status"]) => self.authed(req, |s, r, _| s.handle_status(r)),
            ("POST", ["api", "login"]) => self.handle_login(req),
            ("POST", ["api", "logout"]) => self.handle_logout(req),
            ("GET", ["api", "me"]) => {
                self.authed(req, |_, _, sess| Ok(Response::json(&me_json(&sess.0))))
            }
            ("GET", ["api", "account"]) => {
                self.authed(req, |s, _, sess| s.handle_account(&sess.0))
            }
            ("POST", ["api", "account", "username"]) => {
                self.authed(req, |s, r, sess| s.handle_username_change(r, &sess.0))
            }
            ("POST", ["api", "account", "password"]) => {
                self.authed(req, |s, r, sess| s.handle_password(r, &sess.0))
            }
            ("GET", ["api", "account", "sessions"]) => {
                self.authed(req, |s, _, sess| s.handle_sessions_list(&sess.0, &sess.1))
            }
            ("POST", ["api", "account", "sessions", "revoke"]) => {
                self.authed(req, |s, r, sess| s.handle_session_revoke(r, &sess.0))
            }
            ("POST", ["api", "account", "sessions", "revoke-all"]) => {
                self.authed(req, |s, _, sess| s.handle_sessions_revoke_all(&sess.0))
            }
            ("GET", ["api", "security"]) => {
                self.authed(req, |s, _, _| Ok(Response::json(&s.security_get())))
            }
            ("PUT", ["api", "security"]) => self.authed(req, |s, r, _| s.security_put(r)),
            ("GET", ["api", "setup", "state"]) => Ok(Response::json(&self.setup_state())),
            ("POST", ["api", "setup", "start"]) => self.handle_setup_start(req),
            ("GET", ["api", "setup", "status"]) => Ok(Response::json(
                &self.prog.lock().unwrap().snapshot(),
            )),
            ("GET", ["api", "plugins"]) => self.authed(req, |s, _, _| Ok(Response::json(&s.plugins_list()))),
            ("POST", ["api", "plugins", name, action]) => {
                self.authed(req, move |s, r, _| s.plugin_action(name, action, r))
            }
            ("GET", ["api", "plugins", name, "log"]) => {
                self.authed(req, move |s, _, _| s.plugin_log(name))
            }
            ("DELETE", ["api", "plugins", name]) => {
                self.authed(req, move |s, _, _| s.plugin_delete(name))
            }
            ("GET", ["api", "store"]) => self.authed_opt(req, |s, _| Ok(Response::json(&s.store_list()))),
            ("POST", ["api", "store", name, "install"]) => {
                self.authed(req, move |s, _, _| s.store_install(name))
            }
            ("POST", ["api", "store", "install-url"]) => {
                self.authed(req, |s, r, _| s.store_install_url(r))
            }
            ("GET", ["api", "settings"]) => self.authed(req, |s, _, _| Ok(Response::json(&s.settings_get()))),
            ("PUT", ["api", "settings"]) => self.authed(req, |s, r, _| s.settings_put(r)),
            ("GET", ["api", "log"]) => self.authed(req, |s, _, _| Ok(Response::json(&s.core_log()))),
            ("POST", ["api", "system", "restart"]) => {
                self.authed(req, |_, _, _| Ok(Response::json(&Self::system_restart_msg())))
            }
            (_, ["api", ..]) => Err(Response::json_err(404, "not found")),
            (_, ["p", name, rest @ ..]) => self.gateway_route(req, name, rest),
            _ => self.ui_route(req),
        };

        result.unwrap_or_else(|r| with_security_headers(r))
    }

    fn authed<F>(&self, req: &Request, f: F) -> Result<Response, Response>
    where
        F: FnOnce(
            &Server,
            &Request,
            &(auth::SessionClaims, Session),
        ) -> Result<Response, Response>,
    {
        let sess = self.session_of(req)?;
        f(self, req, &sess)
    }

    /// /api/store 特例：未初始化时对向导开放，初始化后要求登录。
    fn authed_opt<F>(&self, req: &Request, f: F) -> Result<Response, Response>
    where
        F: FnOnce(&Server, &Request) -> Result<Response, Response>,
    {
        if self.db.has_admin() {
            self.session_of(req)?;
        }
        f(self, req)
    }

    // ================= 系统状态 =================

    fn handle_status(&self, _req: &Request) -> Result<Response, Response> {
        let records = self.db.list_plugins();
        let running = records.iter().filter(|p| self.manager.status(&p.name).running).count();
        let cfg = self.cfg.lock().unwrap();
        Ok(Response::json(&serde_json::json!({
            "version": crate::config::VERSION,
            "home": cfg.home,
            "listen_addr": cfg.listen_addr,
            "uptime_seconds": self.start.elapsed().as_secs(),
            "idle_timeout_minutes": self.idle_minutes_effective(),
            "plugins_installed": records.len(),
            "plugins_running": running,
        })))
    }

    fn idle_minutes_effective(&self) -> u64 {
        self.db
            .get_setting("idle_timeout_minutes")
            .and_then(|v| v.parse().ok())
            .unwrap_or(self.manager.idle_secs() / 60)
    }

    // ================= 登录 / 登出 =================

    fn handle_login(self: &Arc<Self>, req: &Request) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let username = body["username"].as_str().unwrap_or("").trim().to_string();
        let password = body["password"].as_str().unwrap_or("");
        let remember = body["remember"].as_bool().unwrap_or(false);
        let is_api = body["api"].as_bool().unwrap_or(false);

        if !self.db.has_admin() {
            return Err(Response::json_err(403, "面板尚未初始化"));
        }
        let (fail_limit, lock_min) = self.security_policy();

        let rem = self.guard.remaining_secs(&username);
        if rem > 0 {
            return Err(Response::json_err(
                423,
                &format!("登录失败次数过多，账号已锁定，请 {} 分钟后再试", rem / 60 + 1),
            ));
        }

        let user = self.db.get_user(&username);
        let ok = user
            .as_ref()
            .map(|u| auth::verify_password(password, &u.salt, &u.password_hash))
            .unwrap_or(false);
        if !ok {
            if self.guard.record_fail(&username, fail_limit, lock_min) {
                return Err(Response::json_err(
                    401,
                    &format!("密码错误次数过多，账号已锁定 {} 分钟", lock_min),
                ));
            }
            return Err(Response::json_err(401, "用户名或密码错误"));
        }
        self.guard.reset(&username);
        let user = user.unwrap();

        // 旧参数哈希自动升级（用户无感知）
        if auth::needs_rehash(&user.salt) {
            if let Ok((salt2, hash2)) = auth::hash_password(password) {
                let _ = self.db.update_password(&user.username, &hash2, &salt2);
                log_line("INFO", &format!("password hash upgraded user={}", user.username));
            }
        }

        let ttl: i64 = if remember { 30 * 24 * 3600 } else { 24 * 3600 };
        let claims = auth::new_session(user.id, &user.username, ttl);
        let token = auth::token_for(&claims, &self.cfg.lock().unwrap().jwt_secret_bytes());
        let _ = self.db.create_session(Session {
            id: 0,
            token_hash: util::sha256_hex_str(&token),
            jti: claims.j.clone(),
            username: user.username.clone(),
            ip: req.peer_ip.clone(),
            user_agent: truncate(req.header("user-agent").unwrap_or(""), 200),
            created_at: util::rfc3339_now(),
            expires_at: util::rfc3339(claims.exp),
            revoked: false,
            api: is_api,
        });
        let _ = self.db.update_last_login(&user.username);

        // 单账号单会话：新登录踢掉该账号其它非 API 会话
        if !is_api {
            let n = self.db.revoke_other_sessions(&user.username, &claims.j).unwrap_or(0);
            if n > 0 {
                log_line(
                    "INFO",
                    &format!("single-session kicked user={} revoked={}", user.username, n),
                );
            }
        }
        self.db.prune_sessions();

        let mut resp = Response::json(&serde_json::json!({
            "ok": true, "username": user.username, "remember": remember,
        }));
        resp.headers.push((
            "Set-Cookie".into(),
            session_cookie(&token, remember, self.secure_cookies(req)),
        ));
        Ok(resp)
    }

    fn secure_cookies(&self, req: &Request) -> bool {
        self.cfg.lock().unwrap().trust_proxy
            && req
                .header("x-forwarded-proto")
                .map(|v| v.eq_ignore_ascii_case("https"))
                .unwrap_or(false)
    }

    fn handle_logout(&self, req: &Request) -> Result<Response, Response> {
        if let Some(token) = header_cookie(req, auth::COOKIE_NAME) {
            let _ = self.db.revoke_by_token_hash(&util::sha256_hex_str(&token));
        }
        let mut resp = Response::json(&serde_json::json!({"ok": true}));
        resp.headers.push((
            "Set-Cookie".into(),
            format!(
                "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
                auth::COOKIE_NAME
            ),
        ));
        Ok(resp)
    }

    // ================= 账户 =================

    fn handle_account(&self, claims: &auth::SessionClaims) -> Result<Response, Response> {
        let (fail_limit, lock_min) = self.security_policy();
        match self.db.get_user(&claims.u) {
            Some(u) => Ok(Response::json(&serde_json::json!({
                "username": u.username,
                "created_at": u.created_at,
                "last_login_at": u.last_login_at,
                "security": {
                    "fail_limit": fail_limit,
                    "lock_minutes": lock_min,
                    "session_max_age_h": 24,
                    "current_session_jti": claims.j,
                },
            }))),
            None => Err(Response::json_err(500, "用户不存在")),
        }
    }

    fn handle_username_change(
        &self,
        req: &Request,
        claims: &auth::SessionClaims,
    ) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let name = body["new_username"].as_str().unwrap_or("").trim().to_string();
        if !(3..=32).contains(&name.chars().count()) {
            return Err(Response::json_err(400, "用户名需为 3-32 个字符"));
        }
        if name.contains([' ', '/', '\t', '\n', '\\', '"', '\'']) {
            return Err(Response::json_err(400, "用户名不能包含空格或特殊字符"));
        }
        if name == claims.u {
            return Err(Response::json_err(400, "新用户名与当前相同"));
        }
        if self.db.get_user(&name).is_some() {
            return Err(Response::json_err(400, "用户名已被占用"));
        }
        self.db.update_username(&claims.u, &name).map_err(internal)?;
        log_line("INFO", &format!("username changed from={} to={}", claims.u, name));
        Ok(Response::json(&serde_json::json!({"ok": true, "username": name})))
    }

    fn handle_password(
        &self,
        req: &Request,
        claims: &auth::SessionClaims,
    ) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let old = body["old_password"].as_str().unwrap_or("");
        let new = body["new_password"].as_str().unwrap_or("");
        if new.chars().count() < 6 {
            return Err(Response::json_err(400, "新密码至少 6 位"));
        }
        let user =
            self.db.get_user(&claims.u).ok_or_else(|| Response::json_err(500, "用户不存在"))?;
        if !auth::verify_password(old, &user.salt, &user.password_hash) {
            return Err(Response::json_err(400, "原密码错误"));
        }
        let (salt, hash) = auth::hash_password(new).map_err(|e| Response::json_err(500, &e))?;
        self.db.update_password(&user.username, &hash, &salt).map_err(internal)?;
        let revoked = self.db.revoke_other_sessions(&user.username, &claims.j).unwrap_or(0);
        log_line("INFO", &format!("password changed user={} revoked={}", user.username, revoked));
        Ok(Response::json(&serde_json::json!({"ok": true, "revoked_sessions": revoked})))
    }

    fn handle_sessions_list(
        &self,
        claims: &auth::SessionClaims,
        current: &Session,
    ) -> Result<Response, Response> {
        let list = self.db.list_sessions(&claims.u);
        let sessions: Vec<serde_json::Value> = list
            .iter()
            .map(|s| {
                serde_json::json!({
                    "jti": s.jti,
                    "ip": s.ip,
                    "user_agent": s.user_agent,
                    "created_at": s.created_at,
                    "expires_at": s.expires_at,
                    "current": s.jti == current.jti,
                    "revoked": s.revoked,
                })
            })
            .collect();
        Ok(Response::json(&serde_json::json!({ "sessions": sessions })))
    }

    fn handle_session_revoke(
        &self,
        req: &Request,
        claims: &auth::SessionClaims,
    ) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let jti = body["jti"].as_str().unwrap_or("");
        if jti.is_empty() {
            return Err(Response::json_err(400, "缺少会话标识"));
        }
        if jti == claims.j {
            return Err(Response::json_err(
                400,
                "不能下线当前登录会话（请使用退出登录）",
            ));
        }
        self.db.revoke_by_jti(jti).map_err(internal)?;
        log_line("INFO", &format!("session revoked user={} jti={}", claims.u, jti));
        Ok(Response::json(&serde_json::json!({"ok": true})))
    }

    fn handle_sessions_revoke_all(&self, claims: &auth::SessionClaims) -> Result<Response, Response> {
        let n = self.db.revoke_other_sessions(&claims.u, &claims.j).map_err(internal)?;
        Ok(Response::json(&serde_json::json!({"ok": true, "revoked": n})))
    }

    // ================= 安全策略 =================

    fn security_get(&self) -> serde_json::Value {
        let (fail_limit, lock_minutes) = self.security_policy();
        serde_json::json!({"fail_limit": fail_limit, "lock_minutes": lock_minutes})
    }

    fn security_put(&self, req: &Request) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let fail = body["fail_limit"].as_u64().unwrap_or(0) as u32;
        let lock = body["lock_minutes"].as_u64().unwrap_or(0);
        if !(1..=100).contains(&fail) {
            return Err(Response::json_err(400, "失败次数上限需在 1-100 之间"));
        }
        if !(1..=1440).contains(&lock) {
            return Err(Response::json_err(400, "锁定时间需在 1-1440 分钟之间"));
        }
        self.db.set_setting("login_fail_limit", &fail.to_string()).map_err(internal)?;
        self.db.set_setting("login_fail_lock_minutes", &lock.to_string()).map_err(internal)?;
        Ok(Response::json(&serde_json::json!({"ok": true})))
    }

    // ================= 初始化向导 =================

    fn setup_state(&self) -> serde_json::Value {
        serde_json::json!({ "configured": self.db.has_admin() })
    }

    fn handle_setup_start(self: &Arc<Self>, req: &Request) -> Result<Response, Response> {
        if self.db.has_admin() {
            return Err(Response::json_err(400, "面板已初始化"));
        }
        let body: serde_json::Value = parse_body(req)?;
        let username = body["username"].as_str().unwrap_or("").trim().to_string();
        let password = body["password"].as_str().unwrap_or("").to_string();
        let plugins: Vec<String> = body["plugins"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if username.len() < 3 || password.len() < 6 {
            return Err(Response::json_err(400, "用户名至少 3 位，密码至少 6 位"));
        }

        {
            let mut p = self.prog.lock().unwrap();
            if p.running {
                return Err(Response::json_err(409, "初始化正在进行中"));
            }
            p.running = true;
            p.done = 0;
            p.total = 1 + plugins.len();
            p.complete = false;
            p.error.clear();
        }

        let server = self.clone();
        std::thread::spawn(move || {
            let step = |srv: &Server, name: &str| {
                let mut p = srv.prog.lock().unwrap();
                p.done += 1;
                p.current = name.to_string();
            };
            let result = (|| -> Result<(), String> {
                let (salt, hash) = auth::hash_password(&password)?;
                server
                    .db
                    .create_user(crate::db::User {
                        id: 0,
                        username: username.clone(),
                        password_hash: hash,
                        salt,
                        created_at: util::rfc3339_now(),
                        last_login_at: String::new(),
                    })
                    .map_err(|e| format!("创建管理员失败: {}", e))?;
                step(&server, "创建管理员账号");
                for name in &plugins {
                    if !installer::catalog_contains(name) {
                        continue;
                    }
                    let home = server.cfg.lock().unwrap().home.clone();
                    installer::install_from_embed(&home, name)
                        .map_err(|e| format!("安装插件 {} 失败: {}", name, e))?;
                    register_plugin(&server, name, "bundled")?;
                    step(&server, name);
                }
                Ok(())
            })();
            let mut p = server.prog.lock().unwrap();
            p.running = false;
            match result {
                Ok(()) => {
                    p.complete = true;
                    p.current = "完成".into();
                }
                Err(e) => p.error = e,
            }
        });

        Ok(Response::json_status(202, &serde_json::json!({"ok": true})))
    }

    // ================= 插件 =================

    fn plugins_list(&self) -> serde_json::Value {
        let home = self.cfg.lock().unwrap().home.clone();
        let items: Vec<serde_json::Value> = self
            .db
            .list_plugins()
            .iter()
            .map(|rec| {
                let menus = Manifest::load(&plugin_dir(&home, &rec.name))
                    .map(|m| m.menus)
                    .unwrap_or_default();
                serde_json::json!({
                    "name": rec.name,
                    "title": rec.title,
                    "version": rec.version,
                    "author": rec.author,
                    "description": rec.description,
                    "keepalive": rec.keepalive,
                    "menus": menus.iter().map(|m| serde_json::json!({
                        "title": m.title, "icon": m.icon, "path": m.path, "section": m.section,
                    })).collect::<Vec<_>>(),
                    "status": self.manager.status(&rec.name),
                })
            })
            .collect();
        serde_json::json!({ "plugins": items })
    }

    fn plugin_action(&self, name: &str, action: &str, req: &Request) -> Result<Response, Response> {
        if !valid_name(name) {
            return Err(Response::json_err(400, "非法插件名"));
        }
        match action {
            "start" => {
                let rt = self.manager.start(name).map_err(|e| Response::json_err(502, &e))?;
                Ok(Response::json(&serde_json::json!({"ok": true, "port": rt.port, "pid": rt.pid})))
            }
            "stop" => {
                self.manager.stop(name).map_err(|e| Response::json_err(400, &e))?;
                Ok(Response::json(&serde_json::json!({"ok": true})))
            }
            "restart" => {
                self.manager.restart(name).map_err(|e| Response::json_err(502, &e))?;
                Ok(Response::json(&serde_json::json!({"ok": true})))
            }
            "keepalive" => {
                let body: serde_json::Value = parse_body(req)?;
                let enabled = body["enabled"].as_bool().unwrap_or(false);
                if !self.db.is_installed(name) {
                    return Err(Response::json_err(404, "插件未安装"));
                }
                self.db.set_keepalive(name, enabled).map_err(internal)?;
                self.manager.apply_keepalive(name, enabled);
                Ok(Response::json(&serde_json::json!({"ok": true, "keepalive": enabled})))
            }
            _ => Err(Response::json_err(404, "未知操作")),
        }
    }

    fn plugin_log(&self, name: &str) -> Result<Response, Response> {
        if !valid_name(name) {
            return Err(Response::json_err(400, "非法插件名"));
        }
        let path = std::path::Path::new(&self.cfg.lock().unwrap().home)
            .join("logs/plugins")
            .join(format!("{}.log", name));
        let log = std::fs::read_to_string(path).unwrap_or_default();
        Ok(Response::json(&serde_json::json!({ "log": log })))
    }

    fn plugin_delete(&self, name: &str) -> Result<Response, Response> {
        if !valid_name(name) {
            return Err(Response::json_err(400, "非法插件名"));
        }
        if !self.db.is_installed(name) {
            return Err(Response::json_err(404, "插件未安装"));
        }
        self.manager.uninstall(name).map_err(|e| Response::json_err(500, &e))?;
        self.db.delete_plugin(name).map_err(internal)?;
        log_line("INFO", &format!("plugin uninstalled plugin={}", name));
        Ok(Response::json(&serde_json::json!({"ok": true})))
    }

    // ================= 商城 =================

    fn store_list(&self) -> serde_json::Value {
        let items: Vec<serde_json::Value> = installer::list_catalog()
            .into_iter()
            .map(|mut it| {
                it.installed = self.db.is_installed(&it.name);
                serde_json::json!({
                    "name": it.name, "title": it.title, "version": it.version,
                    "author": it.author, "description": it.description,
                    "language": it.language, "installed": it.installed,
                })
            })
            .collect();
        serde_json::json!({ "store": items })
    }

    fn store_install(&self, name: &str) -> Result<Response, Response> {
        if !valid_name(name) || !installer::catalog_contains(name) {
            return Err(Response::json_err(404, "商城不存在该插件"));
        }
        let home = self.cfg.lock().unwrap().home.clone();
        installer::install_from_embed(&home, name).map_err(|e| Response::json_err(500, &e))?;
        register_plugin(self, name, "bundled").map_err(|e| Response::json_err(500, &e))?;
        log_line("INFO", &format!("plugin installed plugin={} source=bundled", name));
        Ok(Response::json(&serde_json::json!({"ok": true, "plugin": name})))
    }

    fn store_install_url(&self, req: &Request) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let url = body["url"].as_str().unwrap_or("");
        let sha = body["sha256"].as_str().unwrap_or("");
        if url.is_empty() {
            return Err(Response::json_err(400, "缺少插件包下载地址"));
        }
        let home = self.cfg.lock().unwrap().home.clone();
        let res = installer::install_from_url(&home, url, sha).map_err(internal)?;
        let keep_prev = self.db.get_plugin(&res.name).map(|p| p.keepalive).unwrap_or(false);
        upsert_manifest_record(self, &res.manifest, "remote");
        let _ = self.db.set_keepalive(&res.name, keep_prev || res.manifest.keepalive);
        log_line(
            "INFO",
            &format!(
                "plugin installed from URL plugin={} version={} url={}",
                res.name, res.manifest.version, url
            ),
        );
        Ok(Response::json(
            &serde_json::json!({"ok": true, "plugin": res.name, "version": res.manifest.version}),
        ))
    }

    // ================= 设置 =================

    fn settings_get(&self) -> serde_json::Value {
        let cfg = self.cfg.lock().unwrap();
        let port_map: serde_json::Value = std::fs::read_to_string(
            std::path::Path::new(&cfg.home).join("etc/port-map.json"),
        )
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_else(|| serde_json::json!({}));
        serde_json::json!({
            "version": crate::config::VERSION,
            "home": cfg.home,
            "listen_addr": cfg.listen_addr,
            "idle_timeout_minutes": self.idle_minutes_effective(),
            "theme": self.db.get_setting("theme").filter(|s| !s.is_empty()).unwrap_or_else(|| "sage".into()),
            "lang": self.db.get_setting("lang").unwrap_or_default(),
            "port_pool": format!("{} - {}", cfg.port_lo, cfg.port_hi),
            "port_map": port_map,
        })
    }

    fn settings_put(&self, req: &Request) -> Result<Response, Response> {
        let body: serde_json::Value = parse_body(req)?;
        let mut changed = false;

        if let Some(mins) = body["idle_timeout_minutes"].as_u64() {
            if mins == 0 || mins > 1440 {
                return Err(Response::json_err(400, "空闲退出时间需在 1-1440 分钟之间"));
            }
            self.manager.set_idle(mins * 60);
            self.db.set_setting("idle_timeout_minutes", &mins.to_string()).map_err(internal)?;
            changed = true;
        }
        if let Some(theme) = body["theme"].as_str().filter(|s| !s.is_empty()) {
            if !matches!(theme, "sage" | "ocean" | "rose" | "lilac") {
                return Err(Response::json_err(400, &format!("未知主题: {}", theme)));
            }
            self.db.set_setting("theme", theme).map_err(internal)?;
            changed = true;
        }
        if let Some(lang) = body["lang"].as_str().filter(|s| !s.is_empty()) {
            if lang.len() > 20 {
                return Err(Response::json_err(400, "语言标识过长"));
            }
            self.db.set_setting("lang", lang).map_err(internal)?;
            changed = true;
        }
        let mut need_restart = false;
        if let Some(port) = body["listen_port"].as_u64() {
            if port == 0 || port > 65535 {
                return Err(Response::json_err(400, "端口需在 1-65535 之间"));
            }
            self.cfg
                .lock()
                .unwrap()
                .set_listen_port(port as u16)
                .map_err(|e| Response::json_err(500, &format!("写入配置失败: {}", e)))?;
            need_restart = true;
            changed = true;
        }
        if !changed {
            return Err(Response::json_err(400, "没有可保存的设置项"));
        }
        Ok(Response::json(&serde_json::json!({"ok": true, "need_restart": need_restart})))
    }

    // ================= 日志 / 重启 =================

    fn core_log(&self) -> serde_json::Value {
        let data = std::fs::read_to_string(
            std::path::Path::new(&self.cfg.lock().unwrap().home).join("logs/panel.log"),
        )
        .unwrap_or_default();
        let lines: Vec<&str> = data.lines().collect();
        let start = lines.len().saturating_sub(150);
        serde_json::json!({ "log": lines[start..].join("\n") })
    }

    fn system_restart_msg() -> serde_json::Value {
        let exe = std::env::current_exe().unwrap_or_default();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let _ = std::process::Command::new(exe).arg("restart").spawn();
        });
        serde_json::json!({"ok": true, "msg": "重启已触发，约 2 秒后恢复，请稍后刷新页面"})
    }

    // ================= 插件网关 =================

    fn gateway_route(
        &self,
        req: &Request,
        name: &str,
        rest: &[&str],
    ) -> Result<Response, Response> {
        if !valid_name(name) {
            return Err(Response::json_err(400, "非法插件名"));
        }
        let home = self.cfg.lock().unwrap().home.clone();
        let plugin_path = format!("/{}", rest.join("/"));
        // manifest.auth=none 且路径 /mcp 时免面板登录（插件自鉴权，如 MCP Agent）
        let exempt = Manifest::load(&plugin_dir(&home, name))
            .map(|m| m.auth == "none" && plugin_path == "/mcp")
            .unwrap_or(false);
        if !exempt && !self.logged_in(req) {
            return Err(unauthorized("未登录"));
        }
        let proto = forwarded_proto(req, self.gateway.trust_proxy);
        let orig_host = incoming_host(req, self.gateway.trust_proxy);
        Ok(self.gateway.handle(req, name, &plugin_path, &proto, &orig_host))
    }

    // ================= 前端页面 =================

    fn ui_route(&self, req: &Request) -> Result<Response, Response> {
        if req.method != "GET" && !(req.method == "HEAD") {
            return Err(not_found());
        }
        let configured = self.db.has_admin();
        match req.path.as_str() {
            "/setup" | "/setup/" => {
                if configured {
                    return Err(redirect("/"));
                }
                Ok(serve_asset("setup.html"))
            }
            "/login" | "/login/" => {
                if !configured {
                    return Err(redirect("/setup"));
                }
                if self.logged_in(req) {
                    return Err(redirect("/"));
                }
                Ok(serve_asset("login.html"))
            }
            "/" => {
                if !configured {
                    return Err(redirect("/setup"));
                }
                if !self.logged_in(req) {
                    return Err(redirect("/login"));
                }
                Ok(serve_asset("index.html"))
            }
            other => {
                let name = other.trim_start_matches('/');
                if is_safe_static(name) {
                    if let Some(resp) = try_static(name) {
                        return Ok(resp);
                    }
                }
                Err(not_found())
            }
        }
    }
}

// ================= 独立辅助函数 =================

pub fn me_json(claims: &auth::SessionClaims) -> serde_json::Value {
    serde_json::json!({"username": claims.u, "uid": claims.uid})
}

/// 注册内嵌/手动放入插件到数据库（已登记则跳过）。
pub fn register_plugin(server: &Server, name: &str, source: &str) -> Result<(), String> {
    if server.db.get_plugin(name).is_some() {
        return Ok(());
    }
    let home = server.cfg.lock().unwrap().home.clone();
    let mf = Manifest::load(&plugin_dir(&home, name))?;
    upsert_manifest_record(server, &mf, source);
    if mf.keepalive {
        let _ = server.db.set_keepalive(name, true);
    }
    Ok(())
}

pub fn upsert_manifest_record(server: &Server, mf: &Manifest, source: &str) {
    let _ = server.db.upsert_plugin(PluginRecord {
        name: mf.name.clone(),
        title: mf.title.clone(),
        version: mf.version.clone(),
        author: mf.author.clone(),
        description: mf.description.clone(),
        keepalive: mf.keepalive,
        installed_at: util::rfc3339_now(),
        source: source.into(),
    });
}

pub fn plugin_dir(home: &str, name: &str) -> std::path::PathBuf {
    std::path::Path::new(home).join("plugins").join(name)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

/// 静态资源路径：允许字母数字与 _ . - /（非空、不越权，路径需为 web 内资源）。
fn is_safe_static(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '/')
}

/// db 层 String 错误 → 500 JSON 响应。
pub fn internal(e: String) -> Response {
    Response::json_err(500, &e)
}

fn unauthorized(msg: &str) -> Response {
    Response::json_err(401, msg)
}

fn not_found() -> Response {
    Response::json_err(404, "not found")
}

fn redirect(to: &str) -> Response {
    Response::new(302).header("Location", to)
}

pub fn with_security_headers(mut resp: Response) -> Response {
    let has = |r: &Response, k: &str| r.headers.iter().any(|(a, _)| a.eq_ignore_ascii_case(k));
    if !has(&resp, "X-Frame-Options") {
        resp.headers.push(("X-Frame-Options".into(), "SAMEORIGIN".into()));
    }
    if !has(&resp, "X-Content-Type-Options") {
        resp.headers.push(("X-Content-Type-Options".into(), "nosniff".into()));
    }
    resp
}

pub fn with_hsts(resp: Response) -> Response {
    let mut r = with_security_headers(resp);
    r.headers.push((
        "Strict-Transport-Security".into(),
        "max-age=31536000; includeSubDomains".into(),
    ));
    r
}

pub fn header_cookie<'a>(req: &'a Request, name: &str) -> Option<String> {
    let raw = req.header("cookie")?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn session_cookie(token: &str, remember: bool, secure: bool) -> String {
    let mut c = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax",
        auth::COOKIE_NAME,
        token
    );
    if secure {
        c.push_str("; Secure");
    }
    if remember {
        c.push_str("; Max-Age=2592000");
    }
    c
}

fn forwarded_proto(req: &Request, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(p) = req.header("x-forwarded-proto") {
            return p.to_string();
        }
    }
    "http".to_string()
}

pub fn incoming_host(req: &Request, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(h) = req.header("x-forwarded-host") {
            return h.to_string();
        }
    }
    req.header("host").unwrap_or("").to_string()
}

fn parse_body(req: &Request) -> Result<serde_json::Value, Response> {
    serde_json::from_slice(&req.body)
        .map_err(|_| Response::json_err(400, "请求格式错误"))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 静态资源尝试：命中返回 Response，未命中 None。
fn try_static(name: &str) -> Option<Response> {
    let data = crate::embed::file(name)?;
    let mut resp = Response::new(200).with_body(data.to_vec());
    resp.headers
        .push(("Content-Type".into(), mime_type(name).into()));
    resp.headers.push(("Cache-Control".into(), "no-cache".into()));
    Some(resp)
}

pub fn serve_asset(name: &str) -> Response {
    match try_static(name) {
        Some(r) => r,
        None => not_found(),
    }
}

pub fn mime_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

pub fn log_line(level: &str, msg: &str) {
    crate::manager::log_line(level, msg);
}
