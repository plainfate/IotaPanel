//! REST API handlers, page routing, and middleware.
//! Mirrors the original Go `internal/api` (server.go, handlers.go,
//! account.go, store.go, system.go).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, Request as HttpRequest, Response, StatusCode};
use axum::middleware::Next;
use axum::response::Response as AxResponse;
use axum::Router;
use serde_json::{json, Value};

use crate::auth;
use crate::config::Config;
use crate::db::{self, Db};
use crate::gateway;
use crate::plugins::{self, Manager, Manifest};

type SharedServer = Arc<Server>;

/// Login-failure guard (in-memory; resets on restart, matching the micro-kernel).
struct LoginGuard {
    fails: Mutex<HashMap<String, u32>>,
    until: Mutex<HashMap<String, Instant>>,
}

impl LoginGuard {
    fn new() -> Self {
        LoginGuard { fails: Mutex::new(HashMap::new()), until: Mutex::new(HashMap::new()) }
    }
    fn remaining(&self, user: &str) -> Duration {
        let upto = self.until.lock().unwrap().get(user).copied();
        match upto {
            Some(t) if Instant::now() < t => t - Instant::now(),
            _ => Duration::ZERO,
        }
    }
    fn record_fail(&self, user: &str, limit: u32, lock_min: u64) -> bool {
        let mut f = self.fails.lock().unwrap();
        let n = f.entry(user.to_string()).or_insert(0);
        *n += 1;
        if *n >= limit {
            self.until.lock().unwrap().insert(user.to_string(), Instant::now() + Duration::from_secs(lock_min * 60));
            true
        } else {
            false
        }
    }
    fn reset(&self, user: &str) {
        self.fails.lock().unwrap().remove(user);
        self.until.lock().unwrap().remove(user);
    }
}

struct SetupProgress {
    running: bool,
    done: i32,
    total: i32,
    current: String,
    complete: bool,
    err_msg: String,
}

impl SetupProgress {
    fn new() -> Self {
        SetupProgress { running: false, done: 0, total: 0, current: String::new(), complete: false, err_msg: String::new() }
    }
}

pub struct Server {
    pub cfg: Mutex<Config>,
    pub db: Arc<Db>,
    pub mgr: Arc<Manager>,
    pub start: Instant,
    prog: Mutex<SetupProgress>,
    guard: LoginGuard,
    log_seq: AtomicU64,
}

impl Server {
    pub fn new(cfg: Config, db: Arc<Db>, mgr: Arc<Manager>) -> SharedServer {
        Arc::new(Server {
            cfg: Mutex::new(cfg),
            db,
            mgr,
            start: Instant::now(),
            prog: Mutex::new(SetupProgress::new()),
            guard: LoginGuard::new(),
            log_seq: AtomicU64::new(0),
        })
    }

    /// Extract the authenticated session from the cookie, replicating the Go
    /// `auth` middleware: signature+expiry, then DB persistence + not-revoked.
    fn auth1(&self, headers: &HeaderMap) -> Result<auth::Session, AxResponse> {
        let cookie = match get_cookie(headers) {
            Some(c) => c,
            None => return Err(json_err(401, "未登录")),
        };
        let secret = self.cfg.lock().unwrap().jwt_secret.clone();
        let mut sess = auth::parse_token(&cookie, secret.as_bytes())
            .ok_or_else(|| json_err(401, "会话无效或已过期"))?;
        let rec = self
            .db
            .get_session_by_token_hash(&auth::sha256_hex(&cookie))
            .ok_or_else(|| json_err(401, "会话已失效（可能已被强制下线）"))?;
        if rec.revoked {
            return Err(json_err(401, "会话已失效（可能已被强制下线）"));
        }
        sess.username = rec.username.clone();
        Ok(sess)
    }

    fn logged_in(&self, headers: &HeaderMap) -> bool {
        match get_cookie(headers) {
            Some(c) => auth::parse_token(&c, get_secret(&self.cfg).as_bytes()).is_some(),
            None => false,
        }
    }

    fn security_policy(&self) -> (u32, u64) {
        let mut limit = 5u32;
        let mut lock_min = 15u64;
        if let Some(v) = self.db.get_setting("login_fail_limit") {
            if let Ok(n) = v.parse::<u32>() {
                if n > 0 {
                    limit = n;
                }
            }
        }
        if let Some(v) = self.db.get_setting("login_fail_lock_minutes") {
            if let Ok(n) = v.parse::<u64>() {
                if n > 0 {
                    lock_min = n;
                }
            }
        }
        (limit, lock_min)
    }

    fn info(&self, msg: &str) {
        crate::log_info(&format!("[seq={}] {}", self.log_seq.fetch_add(1, Ordering::SeqCst), msg));
    }
}

fn get_cookie(headers: &HeaderMap) -> Option<String> {
    let all = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in all.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("mp_session=") {
            return Some(v.to_string());
        }
    }
    None
}

fn get_secret(cfg: &Mutex<Config>) -> String {
    cfg.lock().unwrap().jwt_secret.clone()
}

// ---------- JSON helpers ----------

fn json_resp(status: StatusCode, v: Value) -> AxResponse {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(Body::from(v.to_string()))
        .unwrap()
}
fn json_ok(v: Value) -> AxResponse {
    json_resp(StatusCode::OK, v)
}
fn json_err(status: u16, msg: &str) -> AxResponse {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    json_resp(code, json!({"error": msg}))
}

async fn body_json(req: Request<Body>) -> Result<Value, AxResponse> {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .map_err(|_| json_err(400, "请求格式错误"))?;
    serde_json::from_slice(&bytes).map_err(|_| json_err(400, "请求格式错误"))
}

/// Run a blocking closure off the async core and translate errors.
async fn blocking<T, E, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: ToString,
{
    tokio::task::spawn_blocking(|| f().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = 0;
    for (i, _) in s.char_indices() {
        if i >= n {
            break;
        }
        end = i;
    }
    s[..end].to_string()
}

fn client_ip(headers: &HeaderMap) -> String {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let first = first.trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

fn set_session_cookie(headers: &HeaderMap, cfg: &Config, token: &str, remember: bool) -> AxResponse {
    let secure = cfg.trust_proxy
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("https"))
            .unwrap_or(false);
    let mut cookie = format!(
        "mp_session={}; Path=/; HttpOnly; SameSite=Lax",
        token
    );
    if remember {
        cookie.push_str(&format!("; Max-Age={}", 30 * 24 * 3600));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    let mut resp = json_ok(json!({"ok": true}));
    resp.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
    resp
}

fn clear_session_cookie() -> AxResponse {
    let mut resp = json_ok(json!({"ok": true}));
    resp.headers_mut().insert(
        header::SET_COOKIE,
        "mp_session=; Path=/; HttpOnly; Max-Age=-1".parse().unwrap(),
    );
    resp
}

// =============================== MIDDLEWARE ===============================

/// Security headers + CSRF + access logging (outermost layer).
pub async fn mw_all(
    State(s): State<SharedServer>,
    req: HttpRequest<Body>,
    next: Next,
) -> AxResponse {
    // CSRF: state-changing requests need a same-origin Origin (when present).
    match req.method().as_str() {
        "POST" | "PUT" | "DELETE" | "PATCH" => {
            if let Some(origin) = req.headers().get("Origin").and_then(|v| v.to_str().ok()) {
                let host = req
                    .headers()
                    .get(header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                let origin_host = origin
                    .split("://")
                    .nth(1)
                    .map(|h| h.split('/').next().unwrap_or(""))
                    .unwrap_or("");
                if !origin_host.eq_ignore_ascii_case(&host) {
                    return json_resp(StatusCode::FORBIDDEN, json!({"error": "跨站请求被拒绝"}));
                }
            }
        }
        _ => {}
    }

    let resp = next.run(req).await;
    let mut resp = resp;
    resp.headers_mut().insert(header::X_FRAME_OPTIONS, "SAMEORIGIN".parse().unwrap());
    resp.headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    if s.cfg.lock().unwrap().trust_proxy {
        resp.headers_mut().insert(
            header::STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains".parse().unwrap(),
        );
    }
    resp
}

// =============================== PAGES ===============================

fn mime_type(name: &str) -> &'static str {
    if name.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if name.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if name.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if name.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if name.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}

fn serve_page(rel: &str) -> AxResponse {
    match crate::embed::web_file(rel) {
        Some(data) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_type(rel))
            .body(Body::from(data.to_vec()))
            .unwrap(),
        None => Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap(),
    }
}

async fn ui_handler(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if req.method() != axum::http::Method::GET {
        return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap();
    }
    let configured = s.db.has_admin();
    let path = req.uri().path().to_string();
    let q = path.as_str();
    match q {
        "/setup" | "/setup/" => {
            if configured {
                return redirect("/");
            }
            serve_page("setup.html")
        }
        "/login" | "/login/" => {
            if !configured {
                return redirect("/setup");
            }
            if s.logged_in(req.headers()) {
                return redirect("/");
            }
            serve_page("login.html")
        }
        "/" => {
            if !configured {
                return redirect("/setup");
            }
            if !s.logged_in(req.headers()) {
                return redirect("/login");
            }
            serve_page("index.html")
        }
        _ => {
            // static asset under /css /js ...
            let rel = q.trim_start_matches('/');
            match crate::embed::web_file(rel) {
                Some(data) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime_type(rel))
                    .body(Body::from(data.to_vec()))
                    .unwrap(),
                None => Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap(),
            }
        }
    }
}

fn redirect(loc: &str) -> AxResponse {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, loc)
        .body(Body::empty())
        .unwrap()
}

// =============================== GATEWAY ===============================

async fn gateway_handler(
    State(s): State<SharedServer>,
    req: Request<Body>,
) -> AxResponse {
    // Parse `<name>/<path...>` from the /p/ prefix without Path extractor,
    // since the raw Request extractor can't be combined with Path capture.
    let mut rest = req.uri().path().trim_start_matches("/p").to_string();
    if rest.starts_with('/') {
        rest = rest.trim_start_matches('/').to_string();
    }
    let (name, mut plugin_path) = match rest.split_once('/') {
        Some((n, p)) => (n.to_string(), String::from("/") + p),
        None => (rest.to_string(), "/".to_string()),
    };
    if name.is_empty() {
        return json_err(404, "missing plugin name");
    }
    if plugin_path.starts_with("//") {
        plugin_path = plugin_path.trim_start_matches('/').to_string();
        plugin_path = format!("/{}", plugin_path);
    }
    let trust_proxy = s.cfg.lock().unwrap().trust_proxy;
    match gateway::serve(&s.mgr, trust_proxy, req, &name, plugin_path).await {
        Ok(r) => r,
        Err(code) => json_resp(code, json!({"error": "插件连接失败"})),
    }
}

// =============================== ROUTER ===============================

pub fn build_router(s: SharedServer) -> Router {
    use axum::routing::{any, delete, get, post, put};

    let router = Router::new()
        .route("/api/status", get(h_status))
        .route("/api/login", post(h_login))
        .route("/api/logout", post(h_logout))
        .route("/api/me", get(h_me))
        .route("/api/account", get(h_account))
        .route("/api/account/username", post(h_username_change))
        .route("/api/account/password", post(h_account_password))
        .route("/api/account/sessions", get(h_sessions_list))
        .route("/api/account/sessions/revoke", post(h_session_revoke))
        .route("/api/account/sessions/revoke-all", post(h_sessions_revoke_all))
        .route("/api/security", get(h_security_get).put(h_security_put))
        .route("/api/setup/state", get(h_setup_state))
        .route("/api/setup/start", post(h_setup_start))
        .route("/api/setup/status", get(h_setup_status))
        .route("/api/plugins", get(h_plugins_list))
        .route("/api/plugins/{name}/start", post(h_plugin_start))
        .route("/api/plugins/{name}/stop", post(h_plugin_stop))
        .route("/api/plugins/{name}/restart", post(h_plugin_restart))
        .route("/api/plugins/{name}/keepalive", post(h_plugin_keepalive))
        .route("/api/plugins/{name}/log", get(h_plugin_log))
        .route("/api/plugins/{name}", delete(h_plugin_delete))
        .route("/api/store", get(h_store_list))
        .route("/api/store/{name}/install", post(h_store_install))
        .route("/api/store/install-url", post(h_store_install_url))
        .route("/api/settings", get(h_settings_get).put(h_settings_put))
        .route("/api/log", get(h_log))
        .route("/api/system/restart", post(h_system_restart))
        .route("/p/{*rest}", any(gateway_handler))
        .fallback(ui_handler)
        .with_state(s.clone())
        .layer(axum::middleware::from_fn_with_state(s, mw_all));

    router
}

// ============================ STATUS / ME ============================

async fn h_status(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let records = s.db.list_plugins();
    let mut running = 0;
    for p in &records {
        let mgr = s.mgr.clone();
        let name = p.name.clone();
        let st = tokio::task::spawn_blocking(move || mgr.status(&name)).await.unwrap_or_default();
        if st.running {
            running += 1;
        }
    }
    let cfg = s.cfg.lock().unwrap().clone();
    json_ok(json!({
        "version": crate::config::VERSION,
        "home": cfg.home,
        "listen_addr": cfg.listen_addr,
        "uptime_seconds": s.start.elapsed().as_secs(),
        "idle_timeout_minutes": cfg.idle_timeout.as_secs() / 60,
        "plugins_installed": records.len(),
        "plugins_running": running,
    }))
}

async fn h_me(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    match s.auth1(&headers) {
        Ok(sess) => json_ok(json!({"username": sess.username, "uid": sess.uid})),
        Err(e) => e,
    }
}

// ============================ LOGIN / LOGOUT ============================

async fn h_login(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    let headers = req.headers().clone();
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let username = v.get("username").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let remember = v.get("remember").and_then(|x| x.as_bool()).unwrap_or(false);

    if !s.db.has_admin() {
        return json_err(403, "面板尚未初始化");
    }
    let (fail_limit, lock_min) = s.security_policy();
    if let Some(rem) = { let d = s.guard.remaining(&username); (d > Duration::ZERO).then_some(d) } {
        return json_resp(StatusCode::LOCKED, json!({"error": format!(
            "登录失败次数过多，账号已锁定，请 {} 分钟后再试",
            (rem.as_secs() / 60) + 1
        )}));
    }
    let u = s.db.get_user_by_name(&username);
    let ok = match &u {
        Ok(user) => auth::verify_password(&password, &user.salt, &user.password_hash),
        Err(_) => false,
    };
    if !ok {
        let locked = s.guard.record_fail(&username, fail_limit, lock_min);
        return if locked {
            json_resp(StatusCode::UNAUTHORIZED, json!({"error": format!("密码错误次数过多，账号已锁定 {} 分钟", lock_min)}))
        } else {
            json_err(401, "用户名或密码错误")
        };
    }
    s.guard.reset(&username);
    let user = u.unwrap();

    // Rehash legacy (low-iteration) hashes on successful login.
    if auth::needs_rehash(&user.salt) {
        if let Ok(new) = auth::hash_password(&password) {
            let _ = s.db.update_password(&user.username, &new.hash_hex, &new.salt);
            s.info("password hash upgraded");
        }
    }

    let ttl = if remember {
        Duration::from_secs(30 * 24 * 3600)
    } else {
        Duration::from_secs(24 * 3600)
    };
    let cfg = s.cfg.lock().unwrap().clone();
    let sess = auth::new_session(user.id, &user.username, ttl);
    let token = auth::token(&sess, cfg.jwt_secret.as_bytes());
    let _ = s.db.create_session(db::Session {
        id: 0,
        token_hash: auth::sha256_hex(&token),
        jti: sess.jti.clone(),
        username: user.username.clone(),
        ip: client_ip(&headers),
        created_at: String::new(),
        expires_at: chrono::DateTime::from_timestamp(sess.exp, 0)
            .map(|t| t.with_timezone(&chrono::Utc).to_rfc3339())
            .unwrap_or_default(),
        revoked: false,
        user_agent: truncate(
            headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or(""),
            200,
        ),
    });
    let _ = s.db.update_last_login(&user.username);
    let revoked = s.db.revoke_other_sessions(&user.username, &sess.jti).unwrap_or(0);
    if revoked > 0 {
        s.info(&format!("single-session: kicked {} old session(s)", revoked));
    }

    let cookie = cookie_header(&cfg, &token, remember, &headers);
    let mut resp = json_ok(json!({"ok": true, "username": user.username, "remember": remember}));
    resp.headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    resp
}

fn cookie_header(cfg: &Config, token: &str, remember: bool, headers: &HeaderMap) -> String {
    let secure = cfg.trust_proxy
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("https"))
            .unwrap_or(false);
    let mut c = format!("mp_session={}; Path=/; HttpOnly; SameSite=Lax", token);
    if remember {
        c.push_str(&format!("; Max-Age={}", 30 * 24 * 3600));
    }
    if secure {
        c.push_str("; Secure");
    }
    c
}

async fn h_logout(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if let Some(cookie) = get_cookie(req.headers()) {
        let _ = s.db.revoke_session_by_token_hash(&auth::sha256_hex(&cookie));
    }
    clear_session_cookie()
}

// ============================ ACCOUNT ============================

async fn h_account(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    let sess = match s.auth1(&headers) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let u = match s.db.get_user_by_name(&sess.username) {
        Ok(u) => u,
        Err(e) => return json_err(500, &e),
    };
    let (fail_limit, lock_min) = s.security_policy();
    json_ok(json!({
        "username": u.username,
        "created_at": u.created_at,
        "last_login_at": u.last_login_at,
        "security": {
            "fail_limit": fail_limit,
            "lock_minutes": lock_min,
            "session_max_age_h": 24,
            "current_session_jti": sess.jti,
        },
    }))
}

async fn h_username_change(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    let sess = match s.auth1(req.headers()) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let name = v.get("new_username").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if name.len() < 3 || name.len() > 32 {
        return json_err(400, "用户名需为 3-32 个字符");
    }
    if name.contains([' ', '/', '\t', '\n', '\\', '"', '\'']) {
        return json_err(400, "用户名不能包含空格或特殊字符");
    }
    if name == sess.username {
        return json_err(400, "新用户名与当前相同");
    }
    if s.db.get_user_by_name(&name).is_ok() {
        return json_err(400, "用户名已被占用");
    }
    if let Err(e) = s.db.update_username(&sess.username, &name) {
        return json_err(500, &e);
    }
    json_ok(json!({"ok": true, "username": name}))
}

async fn h_account_password(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    let sess = match s.auth1(req.headers()) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let old_pw = v.get("old_password").and_then(|x| x.as_str()).unwrap_or("");
    let new_pw = v.get("new_password").and_then(|x| x.as_str()).unwrap_or("");
    if new_pw.len() < 6 {
        return json_err(400, "新密码至少 6 位");
    }
    let u = s.db.get_user_by_name(&sess.username);
    let ok = match &u {
        Ok(u) => auth::verify_password(old_pw, &u.salt, &u.password_hash),
        Err(_) => false,
    };
    if !ok {
        return json_err(400, "原密码错误");
    }
    let (hash, salt) = match auth::hash_password(new_pw) {
        Ok(h) => (h.hash_hex, h.salt),
        Err(e) => return json_err(500, &e.to_string()),
    };
    let uname = u.unwrap().username;
    if let Err(e) = s.db.update_password(&uname, &hash, &salt) {
        return json_err(500, &e);
    }
    let revoked = s.db.revoke_other_sessions(&uname, &sess.jti).unwrap_or(0);
    json_ok(json!({"ok": true, "revoked_sessions": revoked}))
}

async fn h_sessions_list(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    let sess = match s.auth1(&headers) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let list = s.db.list_sessions(&sess.username);
    let mut out = Vec::new();
    for s2 in list {
        out.push(json!({
            "jti": s2.jti,
            "ip": s2.ip,
            "user_agent": s2.user_agent,
            "created_at": s2.created_at,
            "expires_at": s2.expires_at,
            "current": s2.jti == sess.jti,
            "revoked": s2.revoked,
        }));
    }
    json_ok(json!({"sessions": out}))
}

async fn h_session_revoke(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    let sess = match s.auth1(req.headers()) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let jti = v.get("jti").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if jti.is_empty() {
        return json_err(400, "缺少会话标识");
    }
    if jti == sess.jti {
        return json_err(400, "不能下线当前登录会话（请使用退出登录）");
    }
    if let Err(e) = s.db.revoke_session_by_jti(&jti) {
        return json_err(500, &e);
    }
    json_ok(json!({"ok": true}))
}

async fn h_sessions_revoke_all(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    let sess = match s.auth1(req.headers()) {
        Ok(x) => x,
        Err(e) => return e,
    };
    match s.db.revoke_other_sessions(&sess.username, &sess.jti) {
        Ok(n) => json_ok(json!({"ok": true, "revoked": n})),
        Err(e) => json_err(500, &e),
    }
}

// ============================ SECURITY ============================

async fn h_security_get(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let (fl, lm) = s.security_policy();
    json_ok(json!({"fail_limit": fl, "lock_minutes": lm}))
}

async fn h_security_put(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if let Err(e) = s.auth1(req.headers()) {
        return e;
    }
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let fl = v.get("fail_limit").and_then(|x| x.as_i64()).unwrap_or(0);
    let lm = v.get("lock_minutes").and_then(|x| x.as_i64()).unwrap_or(0);
    if !(1..=100).contains(&fl) {
        return json_err(400, "失败次数上限需在 1-100 之间");
    }
    if !(1..=1440).contains(&lm) {
        return json_err(400, "锁定时间需在 1-1440 分钟之间");
    }
    let _ = s.db.set_setting("login_fail_limit", &fl.to_string());
    let _ = s.db.set_setting("login_fail_lock_minutes", &lm.to_string());
    json_ok(json!({"ok": true}))
}

// ============================ SETUP ============================

async fn h_setup_state(State(s): State<SharedServer>) -> AxResponse {
    json_ok(json!({"configured": s.db.has_admin()}))
}

async fn h_setup_start(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if s.db.has_admin() {
        return json_err(400, "面板已初始化");
    }
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let username = v.get("username").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let plugins: Vec<String> = v
        .get("plugins")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(|x| x.to_string())).collect())
        .unwrap_or_default();
    if username.len() < 3 || password.len() < 6 {
        return json_err(400, "用户名至少 3 位，密码至少 6 位");
    }
    {
        let mut p = s.prog.lock().unwrap();
        if p.running {
            return json_err(409, "初始化正在进行中");
        }
        p.running = true;
        p.done = 0;
        p.complete = false;
        p.err_msg = String::new();
        p.total = 1 + plugins.len() as i32;
    }
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || run_setup(s2, username, password, plugins));
    json_resp(StatusCode::ACCEPTED, json!({"ok": true}))
}

fn run_setup(s: SharedServer, username: String, password: String, plugin_names: Vec<String>) {
    let mut step = |s: &Server, name: &str| {
        let mut p = s.prog.lock().unwrap();
        p.done += 1;
        p.current = name.to_string();
    };
    let mut fail = |s: &Server, msg: &str| {
        let mut p = s.prog.lock().unwrap();
        p.running = false;
        p.err_msg = msg.to_string();
    };
    let _ = &mut step;

    let sp = match auth::hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            fail(&s, &format!("创建管理员失败: {}", e));
            return;
        }
    };
    if let Err(e) = s.db.create_user(db::User {
        id: 0,
        username: username.clone(),
        password_hash: sp.hash_hex,
        salt: sp.salt,
        created_at: String::new(),
        last_login_at: String::new(),
    }) {
        fail(&s, &format!("创建管理员失败: {}", e));
        return;
    }
    {
        let mut p = s.prog.lock().unwrap();
        p.done += 1;
        p.current = "创建管理员账号".to_string();
    }

    for name in plugin_names {
        if !plugins::install::catalog_contains(&name) {
            continue;
        }
        if let Err(e) = install_bundled(&s, &name) {
            fail(&s, &format!("安装插件 {} 失败: {}", name, e));
            return;
        }
        {
            let mut p = s.prog.lock().unwrap();
            p.done += 1;
            p.current = format!("安装 {}", name);
        }
    }

    let mut p = s.prog.lock().unwrap();
    p.running = false;
    p.complete = true;
    p.current = "完成".to_string();
}

async fn h_setup_status(State(s): State<SharedServer>) -> AxResponse {
    let p = s.prog.lock().unwrap();
    json_ok(json!({
        "running": p.running,
        "done": p.done,
        "total": p.total,
        "current": p.current,
        "complete": p.complete,
        "error": p.err_msg,
    }))
}

/// Copy an embedded plugin bundle then register it. Mirrors Go `installBundled`.
fn install_bundled(s: &Server, name: &str) -> Result<(), String> {
    let home = s.cfg.lock().unwrap().home.clone();
    plugins::install::install_from_embed(&home, name)?;
    let mf = plugins::install::manifest_at(&home, name)?;
    let _ = s.db.upsert_plugin(db::PluginRecord {
        name: mf.name.clone(),
        title: mf.title.clone(),
        version: mf.version.clone(),
        author: mf.author.clone(),
        description: mf.description.clone(),
        keepalive: false,
        installed_at: db::now(),
        source: "bundled".to_string(),
    });
    if mf.keepalive {
        let _ = s.db.set_keepalive(&name, true);
    }
    s.info(&format!("plugin installed: {}", name));
    Ok(())
}

// ============================ PLUGINS ============================

async fn h_plugins_list(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let records = s.db.list_plugins();
    let mut out = Vec::new();
    for rec in records {
        let status = {
            let mgr = s.mgr.clone();
            let name = rec.name.clone();
            tokio::task::spawn_blocking(move || mgr.status(&name)).await.unwrap_or_default()
        };
        let home = s.cfg.lock().unwrap().home.clone();
        let mf: Option<Manifest> =
            plugins::load_manifest(&std::path::PathBuf::from(&home).join("plugins").join(&rec.name)).ok();
        out.push(json!({
            "name": rec.name,
            "title": rec.title,
            "version": rec.version,
            "author": rec.author,
            "description": rec.description,
            "keepalive": rec.keepalive,
            "menus": mf.map(|m| m.menus).unwrap_or_default(),
            "status": {
                "running": status.running,
                "port": status.port,
                "pid": status.pid,
                "started_at": status.started_at,
            },
        }));
    }
    json_ok(json!({"plugins": out}))
}

async fn h_plugin_start(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let mgr = s.mgr.clone();
    let n = name.clone();
    match blocking(move || mgr.start(&n)).await {
        Ok(rt) => json_ok(json!({"ok": true, "port": rt.port(), "pid": rt.pid()})),
        Err(e) => json_resp(StatusCode::BAD_GATEWAY, json!({"error": e})),
    }
}

async fn h_plugin_stop(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let mgr = s.mgr.clone();
    match blocking(move || mgr.stop(&name)).await {
        Ok(_) => json_ok(json!({"ok": true})),
        Err(e) => json_err(400, &e),
    }
}

async fn h_plugin_restart(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let mgr = s.mgr.clone();
    let n = name.clone();
    match blocking(move || mgr.restart(&n)).await {
        Ok(_) => json_ok(json!({"ok": true})),
        Err(e) => json_resp(StatusCode::BAD_GATEWAY, json!({"error": e})),
    }
}

async fn h_plugin_keepalive(State(s): State<SharedServer>, Path(name): Path<String>, req: Request<Body>) -> AxResponse {
    if let Err(e) = s.auth1(req.headers()) {
        return e;
    }
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
    if s.db.get_plugin(&name).is_none() {
        return json_err(404, "插件未安装");
    }
    if let Err(e) = s.db.set_keepalive(&name, enabled) {
        return json_err(500, &e);
    }
    {
        let mgr = s.mgr.clone();
        let n = name.clone();
        let _ = tokio::task::spawn_blocking(move || {
            mgr.apply_keepalive(&n, enabled);
        })
        .await;
    }
    json_ok(json!({"ok": true, "keepalive": enabled}))
}

async fn h_plugin_log(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let home = s.cfg.lock().unwrap().home.clone();
    let path = std::path::PathBuf::from(&home).join("logs").join("plugins").join(format!("{}.log", name));
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    json_ok(json!({"log": data}))
}

async fn h_plugin_delete(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    if s.db.get_plugin(&name).is_none() {
        return json_err(404, "插件未安装");
    }
    {
        let mgr = s.mgr.clone();
        let n = name.clone();
        if let Err(e) = blocking(move || mgr.uninstall(&n)).await {
            return json_err(500, &e);
        }
    }
    if let Err(e) = s.db.delete_plugin(&name) {
        return json_err(500, &e);
    }
    json_ok(json!({"ok": true}))
}

// ============================ STORE ============================

async fn h_store_list(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    let configured = s.db.has_admin();
    if configured && !s.logged_in(&headers) {
        return json_err(401, "未登录");
    }
    let catalog = plugins::install::list_catalog().unwrap_or_default();
    let mut stored: Vec<Value> = Vec::new();
    for mut item in catalog {
        item.installed = s.db.get_plugin(&item.name).is_some();
        stored.push(json!({
            "name": item.name,
            "title": item.title,
            "version": item.version,
            "author": item.author,
            "description": item.description,
            "language": item.language,
            "installed": item.installed,
        }));
    }
    json_ok(json!({"store": stored}))
}

async fn h_store_install(State(s): State<SharedServer>, Path(name): Path<String>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    if !plugins::install::catalog_contains(&name) {
        return json_err(404, "商城不存在该插件");
    }
    match install_bundled(&s, &name) {
        Ok(_) => json_ok(json!({"ok": true, "plugin": name})),
        Err(e) => json_err(500, &e),
    }
}

async fn h_store_install_url(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if let Err(e) = s.auth1(req.headers()) {
        return e;
    }
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let url = v.get("url").and_then(|x| x.as_str()).unwrap_or("");
    let sha256 = v.get("sha256").and_then(|x| x.as_str()).unwrap_or("");
    if url.is_empty() {
        return json_err(400, "缺少插件包下载地址");
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return json_err(400, "仅支持 http/https 下载地址");
    }
    let (name, files) = match install_url_download(url, sha256).await {
        Ok(x) => x,
        Err((code, msg)) => return json_resp(code, json!({"error": msg})),
    };
    let home = s.cfg.lock().unwrap().home.clone();
    if let Err(e) = plugins::install::write_package(&home, &name, &files) {
        return json_err(500, &e);
    }
    let keepalive = s.db.get_plugin(&name).map(|p| p.keepalive).unwrap_or(false);
    let mf = match plugins::install::manifest_at(&home, &name) {
        Ok(m) => m,
        Err(e) => return json_err(500, &format!("manifest 解析失败: {}", e)),
    };
    let _ = s.db.upsert_plugin(db::PluginRecord {
        name: mf.name.clone(),
        title: mf.title.clone(),
        version: mf.version.clone(),
        author: mf.author.clone(),
        description: mf.description.clone(),
        keepalive: false,
        installed_at: db::now(),
        source: "remote".to_string(),
    });
    let _ = s.db.set_keepalive(&name, keepalive || mf.keepalive);
    json_ok(json!({"ok": true, "plugin": name, "version": mf.version}))
}

async fn install_url_download(
    url: &str,
    sha256: &str,
) -> Result<(String, std::collections::HashMap<String, Vec<u8>>), (StatusCode, String)> {
    let data = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("客户端初始化失败: {}", e)))?
        .get(url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("下载失败: {}", e)))?
        .error_for_status()
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("下载失败: {}", e)))?
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("读取下载内容失败: {}", e)))?
        .to_vec();
    if !sha256.is_empty() {
        let sum = auth::sha256_hex(std::str::from_utf8(&data).unwrap_or(""));
        if sum != sha256.to_lowercase() {
            return Err((StatusCode::BAD_REQUEST, "SHA256 校验失败，包可能被篡改或下载不完整".into()));
        }
    }
    match plugins::install::unpack_plugin_package(&data) {
        Ok((n, f)) => Ok((n, f)),
        Err(e) => Err((StatusCode::BAD_REQUEST, format!("插件包解析失败: {}", e))),
    }
}

// ============================ SETTINGS ============================

async fn h_settings_get(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let cfg = s.cfg.lock().unwrap().clone();
    let mut idle_min = cfg.idle_timeout.as_secs() / 60;
    if let Some(v) = s.db.get_setting("idle_timeout_minutes") {
        if let Ok(n) = v.parse::<u64>() {
        idle_min = n;
    }
    }
    let theme = s.db.get_setting("theme").filter(|t| !t.is_empty()).unwrap_or_else(|| "sage".into());
    let lang = s.db.get_setting("lang").unwrap_or_default();
    let port_map: Value = std::fs::read_to_string(std::path::PathBuf::from(&cfg.home).join("etc").join("port-map.json"))
        .and_then(|d| Ok(serde_json::from_str(&d).unwrap_or(Value::Null)))
        .unwrap_or(Value::Null);
    json_ok(json!({
        "version": crate::config::VERSION,
        "home": cfg.home,
        "listen_addr": cfg.listen_addr,
        "idle_timeout_minutes": idle_min,
        "theme": theme,
        "lang": lang,
        "port_pool": format!("{} - {}", cfg.port_lo, cfg.port_hi),
        "port_map": port_map,
    }))
}

const THEME_NAMES: [&str; 4] = ["sage", "ocean", "rose", "lilac"];

async fn h_settings_put(State(s): State<SharedServer>, req: Request<Body>) -> AxResponse {
    if let Err(e) = s.auth1(req.headers()) {
        return e;
    }
    let v = match body_json(req).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let idle_min = v.get("idle_timeout_minutes").and_then(|x| x.as_i64()).unwrap_or(0);
    let theme = v.get("theme").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let lang = v.get("lang").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let listen_port = v.get("listen_port").and_then(|x| x.as_i64()).unwrap_or(0);

    let mut changed = false;
    let mut need_restart = false;

    if idle_min > 0 {
        if !(1..=1440).contains(&idle_min) {
            return json_err(400, "空闲退出时间需在 1-1440 分钟之间");
        }
        let d = Duration::from_secs((idle_min as u64) * 60);
        s.mgr.set_idle(d);
        let _ = s.db.set_setting("idle_timeout_minutes", &idle_min.to_string());
        changed = true;
    }

    if !theme.is_empty() {
        if !THEME_NAMES.contains(&theme.as_str()) {
            return json_err(400, &format!("未知主题: {}", theme));
        }
        let _ = s.db.set_setting("theme", &theme);
        changed = true;
    }

    if !lang.is_empty() {
        if lang.len() > 20 {
            return json_err(400, "语言标识过长");
        }
        let _ = s.db.set_setting("lang", &lang);
        changed = true;
    }

    if listen_port > 0 {
        if !(1..=65535).contains(&listen_port) {
            return json_err(400, "端口需在 1-65535 之间");
        }
        let current_addr = s.cfg.lock().unwrap().listen_addr.clone();
        let new_addr = replace_port(&current_addr, listen_port);
        let home = s.cfg.lock().unwrap().home.clone();
        if let Err(e) = crate::config::set_env_var(&home, "LISTEN_ADDR", &new_addr) {
            return json_err(500, &format!("写入配置失败: {}", e));
        }
        s.cfg.lock().unwrap().listen_addr = new_addr;
        need_restart = true;
        changed = true;
    }

    if !changed {
        return json_err(400, "没有可保存的设置项");
    }
    json_ok(json!({"ok": true, "need_restart": need_restart}))
}

fn replace_port(addr: &str, port: i64) -> String {
    // addr may be ":8787" or "127.0.0.1:8787" or "[::]:8787"
    let idx = addr.rfind(':').unwrap_or(addr.len());
    let host = &addr[..idx];
    if host.contains('[') {
        format!("[{}]{}", host.trim_start_matches('[').trim_end_matches(']'), format!(":{}", port))
    } else if host.is_empty() {
        format!(":{}", port)
    } else {
        format!("{}:{}", host, port)
    }
}

// ============================ LOG / SYSTEM ============================

async fn h_log(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    let home = s.cfg.lock().unwrap().home.clone();
    let path = std::path::PathBuf::from(&home).join("logs").join("panel.log");
    let data = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = data.trim_end_matches('\n').split('\n').collect();
    if lines.len() > 150 {
        lines.drain(..lines.len() - 150);
    }
    json_ok(json!({"log": lines.join("\n")}))
}

async fn h_system_restart(State(s): State<SharedServer>, headers: HeaderMap) -> AxResponse {
    if let Err(e) = s.auth1(&headers) {
        return e;
    }
    // Mirrors Go: async re-exec of `panel restart` after a short delay.
    let exe = std::env::current_exe().ok();
    tokio::task::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(exe) = exe {
            let _ = std::process::Command::new(exe).arg("restart").spawn();
        }
    });
    json_ok(json!({"ok": true, "msg": "重启已触发，约 2 秒后恢复，请稍后刷新页面"}))
}