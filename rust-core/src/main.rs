// SPDX-License-Identifier: Apache-2.0
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! IotaPanel Rust 微内核核心（v0.2：认证版）。
//! 与 Go 版数据完全兼容：共用 data/panel.json（用户/会话）与 etc/.env（JWT_SECRET），
//! 同一安装目录可在 Go/Rust 核心间切换而不丢失登录态。
//! 认证：PBKDF2-SHA256 口令校验、HMAC-SHA256 会话令牌、cookie 会话、401、CSRF Origin 校验。
//! 已实现：插件契约（manifest+环境变量+port-map+网关）、认证会话、登录/登出、内嵌登录页。

use std::collections::HashMap;
use std::env;
use std::fs;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::client::legacy::{Client as HyperClient, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;
const COOKIE: &str = "mp_session";

// ---------- 存储（与 Go 版 panel.json 兼容） ----------

#[derive(Serialize, Deserialize, Default, Clone)]
struct PanelData {
    users: Vec<User>,
    #[serde(default)]
    plugins: Vec<serde_json::Value>,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    settings: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct User {
    id: i64,
    username: String,
    password_hash: String,
    salt: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    last_login_at: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Session {
    id: i64,
    token_hash: String,
    jti: String,
    username: String,
    #[serde(default)]
    ip: String,
    #[serde(default)]
    user_agent: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    expires_at: String,
    #[serde(default)]
    revoked: bool,
    #[serde(default)]
    api: bool,
}

fn panel_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("data").join("panel.json")
}

fn load_panel(home: &str) -> PanelData {
    let data = fs::read_to_string(panel_path(home)).unwrap_or_default();
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_panel(home: &str, data: &PanelData) {
    let p = panel_path(home);
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(data) {
        let _ = fs::write(p, json);
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn rand_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = getrandom::getrandom(&mut b);
    hex::encode(b)
}

// ---------- 口令校验（Go: salt 为 "迭代次数:hex"，旧版纯 hex=10万次） ----------

fn parse_salt(salt: &str) -> (u32, Vec<u8>) {
    if let Some((n, rest)) = salt.split_once(':') {
        if let Ok(iter) = n.parse::<u32>() {
            if let Ok(b) = hex::decode(rest) {
                return (iter, b);
            }
        }
    }
    (100_000, hex::decode(salt).unwrap_or_default())
}

fn verify_password(pw: &str, salt: &str, hash: &str) -> bool {
    let (iter, saltb) = parse_salt(salt);
    if saltb.is_empty() {
        return false;
    }
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(pw.as_bytes(), &saltb, iter, &mut out);
    hex::encode(out).as_bytes().ct_eq(hash.as_bytes()).into()
}

// ---------- 会话令牌（Go: base64url(json{uid,u,exp,j}).HMAC-SHA256） ----------

fn verify_token(token: &str, secret: &[u8]) -> Option<(String, String)> {
    let (payload, sig) = token.split_once('.')?;
    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(payload.as_bytes());
    let expect = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let sig_ok: bool = expect.as_bytes().ct_eq(sig.as_bytes()).into();
    if !sig_ok {
        return None;
    }
    let raw = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let jti = v.get("j")?.as_str()?.to_string();
    let user = v.get("u")?.as_str()?.to_string();
    let exp = v.get("exp")?.as_i64()?;
    if exp < now_secs() {
        return None;
    }
    Some((jti, user))
}

fn secret_from_env(home: &str) -> Vec<u8> {
    if let Ok(s) = env::var("JWT_SECRET") {
        if !s.is_empty() {
            return s.into_bytes();
        }
    }
    if let Ok(envfile) = fs::read_to_string(PathBuf::from(home).join("etc").join(".env")) {
        for line in envfile.lines() {
            if let Some(v) = line.trim().strip_prefix("JWT_SECRET=") {
                return v.trim().trim_matches('"').trim_matches('\'').as_bytes().to_vec();
            }
        }
    }
    Vec::new()
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    for (k, v) in headers.iter() {
        if k.as_str().eq_ignore_ascii_case("cookie") {
            let s = v.to_str().ok()?;
            for part in s.split(';') {
                let part = part.trim();
                if let Some((n, val)) = part.split_once('=') {
                    if n.trim() == name {
                        return Some(val.trim().to_string());
                    }
                }
            }
        }
    }
    None
}

fn authed(home: &str, headers: &axum::http::HeaderMap, secret: &[u8]) -> bool {
    let token = match cookie_value(headers, COOKIE) {
        Some(t) => t,
        None => return false,
    };
    let (_jti, user) = match verify_token(&token, secret) {
        Some(v) => v,
        None => return false,
    };
    let panel = load_panel(home);
    let th = sha256_hex(&token);
    panel
        .sessions
        .iter()
        .any(|s| s.token_hash == th && !s.revoked && s.username == user)
}

fn unauthorized() -> Response<Body> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(Body::from("{\"error\":\"未登录\"}"))
        .unwrap()
}

fn json_resp(code: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(code)
        .header("Content-Type", "application/json; charset=utf-8")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn origin_ok(origin: &str, host: &str) -> bool {
    // 用 URL 解析取 host:port（带默认端口归一化），避免纯字符串前缀比较的边界问题
    let Ok(u) = url::Url::parse(origin) else {
        return false;
    };
    let Some(oh) = u.host_str() else {
        return false;
    };
    let oa = match u.port_or_known_default() {
        Some(p) => format!("{}:{}", oh, p),
        None => oh.to_string(),
    };
    oa == host
}

// ---------- 登录 / 登出 ----------

fn handle_login(app: &App, headers: &axum::http::HeaderMap, body: &str) -> Response<Body> {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
        if !origin_ok(origin, host) {
            return json_resp(StatusCode::FORBIDDEN, "{\"error\":\"跨站请求被拒绝\"}");
        }
    }
    let v: serde_json::Value =
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let user = v.get("username").and_then(|x| x.as_str()).unwrap_or("");
    let pass = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    let is_api = v.get("api").and_then(|x| x.as_bool()).unwrap_or(false);
    let secret = secret_from_env(&app.home);
    if secret.is_empty() {
        return json_resp(StatusCode::INTERNAL_SERVER_ERROR, "{\"error\":\"未配置 JWT_SECRET\"}");
    }
    let mut panel = load_panel(&app.home);
    let u = match panel.users.iter().find(|u| u.username == user) {
        Some(u) => u.clone(),
        None => return json_resp(StatusCode::UNAUTHORIZED, "{\"error\":\"用户名或密码错误\"}"),
    };
    if !verify_password(pass, &u.salt, &u.password_hash) {
        return json_resp(StatusCode::UNAUTHORIZED, "{\"error\":\"用户名或密码错误\"}");
    }
    let jti = rand_hex(16);
    let exp = now_secs() + 24 * 3600;
    let payload = serde_json::json!({"uid": u.id, "u": u.username, "exp": exp, "j": jti});
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    let mut mac = HmacSha256::new_from_slice(&secret).unwrap();
    mac.update(payload_b64.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let token = format!("{}.{}", payload_b64, sig);
    let th = sha256_hex(&token);
    if panel.sessions.len() > 500 {
        panel.sessions.retain(|s| !s.revoked);
    }
    // 单账号单会话：新登录踢掉该账号其它非 API 会话（与 Go 版一致）
    if !is_api {
        for s in panel.sessions.iter_mut() {
            if s.username == u.username && !s.api {
                s.revoked = true;
            }
        }
    }
    let next_id = panel.sessions.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    panel.sessions.push(Session {
        id: next_id,
        token_hash: th,
        jti,
        username: u.username.clone(),
        created_at: now_secs().to_string(),
        expires_at: exp.to_string(),
        api: is_api,
        ..Default::default()
    });
    save_panel(&app.home, &panel);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Set-Cookie", format!("{}={}; Path=/; HttpOnly; SameSite=Lax", COOKIE, token))
        .body(Body::from(format!("{{\"ok\":true,\"username\":\"{}\"}}", u.username)))
        .unwrap()
}

fn handle_logout(app: &App, headers: &axum::http::HeaderMap) -> Response<Body> {
    if let Some(token) = cookie_value(headers, COOKIE) {
        let th = sha256_hex(&token);
        let mut panel = load_panel(&app.home);
        let mut changed = false;
        for s in panel.sessions.iter_mut() {
            if s.token_hash == th && !s.revoked {
                s.revoked = true;
                changed = true;
            }
        }
        if changed {
            save_panel(&app.home, &panel);
        }
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("Set-Cookie", format!("{}=", COOKIE))
        .body(Body::from("{\"ok\":true}"))
        .unwrap()
}

// ---------- 初始化向导 ----------

fn configured(home: &str) -> bool {
    load_panel(home).users.iter().any(|u| !u.password_hash.is_empty())
}

fn handle_setup_state(home: &str) -> Response<Body> {
    json_resp(StatusCode::OK, &format!("{{\"configured\":{}}}", configured(home)))
}

fn handle_setup_start(app: &App, body: &str) -> Response<Body> {
    if configured(&app.home) {
        return json_resp(StatusCode::FORBIDDEN, "{\"error\":\"面板已初始化\"}");
    }
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let user = v.get("username").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let pass = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    if user.is_empty() || pass.len() < 6 {
        return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"用户名不能为空，密码至少 6 位\"}");
    }
    // 生成/写入 JWT_SECRET
    let secret = secret_from_env(&app.home);
    if secret.is_empty() {
        let secret_val = rand_hex(32);
        let env_path = PathBuf::from(&*app.home).join("etc").join(".env");
        if let Some(dir) = env_path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut env = fs::read_to_string(&env_path).unwrap_or_default();
        if !env.contains("JWT_SECRET=") {
            if !env.is_empty() && !env.ends_with('\n') {
                env.push('\n');
            }
            env.push_str(&format!("JWT_SECRET={}\n", secret_val));
            let _ = fs::write(&env_path, env);
        }
    }
    // 创建管理员（PBKDF2-SHA256 60万次，Go 格式）
    let mut salt = [0u8; 16];
    let _ = getrandom::getrandom(&mut salt);
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(pass.as_bytes(), &salt, 600_000, &mut out);
    let mut panel = load_panel(&app.home);
    let next_id = panel.users.iter().map(|u| u.id).max().unwrap_or(0) + 1;
    panel.users.push(User {
        id: next_id,
        username: user.clone(),
        password_hash: hex::encode(out),
        salt: format!("600000:{}", hex::encode(salt)),
        created_at: now_secs().to_string(),
        ..Default::default()
    });
    save_panel(&app.home, &panel);
    json_resp(StatusCode::OK, &format!("{{\"ok\":true,\"username\":\"{}\"}}", user))
}

fn handle_password(app: &App, headers: &axum::http::HeaderMap, body: &str) -> Response<Body> {
    let secret = secret_from_env(&app.home);
    let token = match cookie_value(headers, COOKIE) {
        Some(t) => t,
        None => return unauthorized(),
    };
    let (_jti, user) = match verify_token(&token, &secret) {
        Some(v) => v,
        None => return unauthorized(),
    };
    let th = sha256_hex(&token);
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let old = v.get("old_password").and_then(|x| x.as_str()).unwrap_or("");
    let new = v.get("new_password").and_then(|x| x.as_str()).unwrap_or("");
    if new.len() < 6 {
        return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"新密码至少 6 位\"}");
    }
    let mut panel = load_panel(&app.home);
    let idx = match panel.users.iter().position(|u| u.username == user) {
        Some(i) => i,
        None => return unauthorized(),
    };
    if !verify_password(old, &panel.users[idx].salt, &panel.users[idx].password_hash) {
        return json_resp(StatusCode::UNAUTHORIZED, "{\"error\":\"原密码错误\"}");
    }
    let mut salt = [0u8; 16];
    let _ = getrandom::getrandom(&mut salt);
    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(new.as_bytes(), &salt, 600_000, &mut out);
    panel.users[idx].password_hash = hex::encode(out);
    panel.users[idx].salt = format!("600000:{}", hex::encode(salt));
    // 改密吊销其它会话（含 API 会话，与 Go 版一致）
    for s in panel.sessions.iter_mut() {
        if s.username == user && s.token_hash != th {
            s.revoked = true;
        }
    }
    save_panel(&app.home, &panel);
    json_resp(StatusCode::OK, "{\"ok\":true}")
}

// ---------- 内嵌登录页 ----------

const LOGIN_HTML: &str = r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>IotaPanel · 登录</title>
<style>body{font-family:system-ui;background:#f6f4ef;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}
.card{background:#fff;padding:32px;border-radius:12px;box-shadow:0 4px 24px rgba(0,0,0,.08);width:320px}
h1{font-size:20px;margin:0 0 4px}input{width:100%;box-sizing:border-box;padding:10px;margin:10px 0;border:1px solid #ddd;border-radius:6px}
button{width:100%;padding:10px;background:#5e7f58;color:#fff;border:0;border-radius:6px;cursor:pointer;font-size:14px}
#err{color:#c46a5a;font-size:13px;min-height:18px;margin-top:6px}</style></head><body>
<div class="card"><h1>IotaPanel</h1><div id="err"></div>
<input id="u" placeholder="用户名"><input id="p" type="password" placeholder="密码" onkeydown="if(event.key==='Enter')login()">
<button onclick="login()">登录</button></div>
<script>
async function login(){const e=document.getElementById('err');e.textContent='';
const r=await fetch('/api/login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:document.getElementById('u').value,password:document.getElementById('p').value})});
const d=await r.json();if(!r.ok){e.textContent=d.error||'登录失败';return;}location.href='/';}
</script></body></html>"#;

const SETUP_HTML: &str = r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>IotaPanel · 初始化</title>
<style>body{font-family:system-ui;background:#f6f4ef;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}
.card{background:#fff;padding:32px;border-radius:12px;box-shadow:0 4px 24px rgba(0,0,0,.08);width:320px}
h1{font-size:20px;margin:0 0 4px}.sub{color:#7e8a94;font-size:13px;margin-bottom:12px}
input{width:100%;box-sizing:border-box;padding:10px;margin:8px 0;border:1px solid #ddd;border-radius:6px}
button{width:100%;padding:10px;background:#5e7f58;color:#fff;border:0;border-radius:6px;cursor:pointer}
#err{color:#c46a5a;font-size:13px;min-height:18px;margin-top:6px}</style></head><body>
<div class="card"><h1>IotaPanel 初始化</h1><div class="sub">创建管理员账号（用于登录面板）</div>
<div id="err"></div>
<input id="u" placeholder="管理员用户名" value="admin">
<input id="p" type="password" placeholder="密码（至少 6 位）">
<input id="p2" type="password" placeholder="确认密码">
<button onclick="go()">创建并登录</button></div>
<script>
async function go(){const e=document.getElementById('err');e.textContent='';
const p1=document.getElementById('p').value,p2=document.getElementById('p2').value;
if(p1!==p2){e.textContent='两次密码不一致';return;}
const r=await fetch('/api/setup/start',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({username:document.getElementById('u').value,password:p1})});
const d=await r.json();if(!r.ok){e.textContent=d.error||'初始化失败';return;}location.href='/';}
</script></body></html>"#;

const ADMIN_HTML: &str = r#"<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>IotaPanel</title>
<style>
:root{--bg:#f6f4ef;--panel:#fff;--accent:#5e7f58;--text:#33393f;--muted:#7e8a94;--border:#e4e0d6}
*{box-sizing:border-box;margin:0;padding:0}
body{font-family:system-ui;background:var(--bg);color:var(--text);display:flex;height:100vh}
#side{width:212px;background:#efece3;border-right:1px solid var(--border);display:flex;flex-direction:column;padding:14px 0;overflow:auto}
#brand{padding:4px 18px 14px;font-size:17px;font-weight:700}
#side a{display:block;padding:8px 18px;color:var(--text);text-decoration:none;border-left:3px solid transparent;cursor:pointer;font-size:13px}
#side a:hover{background:#f1f2ec}
#side a.on{border-color:var(--accent);background:#f1f2ec;font-weight:600}
#side .sec{font-size:11px;color:var(--muted);padding:12px 18px 4px}
#main{flex:1;display:flex;flex-direction:column;min-width:0}
#top{height:48px;background:var(--panel);border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between;padding:0 18px;flex:none}
#frameWrap{flex:1;display:none;min-height:0}
#frame{width:100%;height:100%;border:0}
.view{flex:1;overflow:auto;padding:22px;display:none}
.view.on{display:block}
.card{background:var(--panel);border:1px solid var(--border);border-radius:10px;padding:18px;margin-bottom:14px}
h2{font-size:16px;margin-bottom:12px}
p{margin:6px 0;font-size:13px;color:var(--muted)}
table{width:100%;border-collapse:collapse;font-size:13px}
td,th{padding:8px 10px;border-bottom:1px solid var(--border);text-align:left}
button{padding:5px 12px;border:1px solid var(--border);background:var(--panel);border-radius:6px;cursor:pointer;font-size:12px}
button.p{background:var(--accent);color:#fff;border-color:var(--accent)}
input{width:100%;padding:9px;border:1px solid var(--border);border-radius:6px;margin:6px 0;font-size:13px}
.badge{font-size:11px;padding:2px 8px;border-radius:10px}
.badge.on{background:#e6efe4;color:#4a6b45}.badge.off{background:#f4e9e7;color:#9c4a42}
#msg{color:#c46a5a;font-size:13px;min-height:18px}
</style></head><body>
<div id="side">
 <div id="brand">IotaPanel</div>
 <div class="sec">管理</div>
 <a onclick="show('dash')" class="on" id="nav-dash">📊 概览</a>
 <a onclick="show('plugs')" id="nav-plugs">🧩 插件</a>
 <a onclick="show('acct')" id="nav-acct">🔑 账户</a>
 <div class="sec">插件菜单</div><div id="pmenus"></div>
 <div style="flex:1"></div>
 <a onclick="logout()">🚪 退出登录</a>
</div>
<div id="main">
 <div id="top"><div id="ptitle" style="font-weight:600">概览</div><div id="who" style="font-size:13px;color:var(--muted)"></div></div>
 <div id="frameWrap"><iframe id="frame"></iframe></div>
 <div class="view on" id="v-dash"><div class="card" id="dashbox">加载中…</div></div>
 <div class="view" id="v-plugs"><div class="card" id="plugsbox">加载中…</div></div>
 <div class="view" id="v-acct"><div class="card"><h2>修改密码</h2><div id="msg"></div>
  <input id="op" type="password" placeholder="原密码"><input id="np" type="password" placeholder="新密码（至少 6 位）"><input id="np2" type="password" placeholder="确认新密码">
  <button class="p" onclick="chpw()">保存</button></div></div>
</div>
<script>
let PLUGINS=[],VERSION='0.4.1',HOME='';
async function api(p,o){const r=await fetch(p,o);const d=await r.json().catch(()=>({}));if(r.status===401){location.href='/login';}if(!r.ok)throw new Error(d.error||('HTTP '+r.status));return d;}
async function boot(){
 try{const me=await api('/api/me');document.getElementById('who').textContent=me.username;}catch(e){}
 try{const o=await api('/api/overview');VERSION=o.version;HOME=o.home;}catch(e){}
 await loadPlugins();show('dash');
}
async function loadPlugins(){
 try{PLUGINS=(await api('/api/plugins')).plugins||[];}catch(e){PLUGINS=[];}
 const pm=document.getElementById('pmenus');pm.innerHTML='';
 for(const p of PLUGINS){for(const m of (p.menus||[])){
   const a=document.createElement('a');
   a.innerHTML=(m.icon||'')+' '+m.title;
   a.onclick=()=>openPlugin(p.name,m.path||'/');
   pm.appendChild(a);}}
 renderDash();renderPlugs();
}
function openPlugin(name,path){hideViews();document.getElementById('frame').src='/p/'+name+path;
 document.getElementById('frameWrap').style.display='flex';document.getElementById('ptitle').textContent=name;}
function hideViews(){document.querySelectorAll('.view').forEach(v=>v.classList.remove('on'));document.getElementById('frameWrap').style.display='none';}
function show(v){hideViews();document.getElementById('v-'+v).classList.add('on');
 document.getElementById('ptitle').textContent={dash:'概览',plugs:'插件',acct:'账户'}[v];
 document.querySelectorAll('#side a').forEach(a=>a.classList.remove('on'));document.getElementById('nav-'+v).classList.add('on');
 if(v==='plugs')loadPlugins();}
function renderDash(){const r=PLUGINS.filter(p=>p.status.running).length;
 document.getElementById('dashbox').innerHTML='<h2>概览</h2><p>版本：'+VERSION+'</p><p>安装目录：'+HOME+'</p>'
 +'<p>插件：'+PLUGINS.length+' 个，运行中 '+r+' 个</p>';}
function renderPlugs(){let h='<h2>插件</h2><table><tr><th>名称</th><th>状态</th><th>端口</th><th>保活</th><th>操作</th></tr>';
 for(const p of PLUGINS){h+='<tr><td>'+esc(p.title)+' <small>'+esc(p.name)+'</small></td>'
  +'<td><span class="badge '+(p.status.running?'on':'off')+'">'+(p.status.running?'运行中':'已停止')+'</span></td>'
  +'<td>'+(p.status.port||'-')+'</td>'
  +'<td><input type="checkbox" '+(p.keepalive?'checked':'')+' onchange="kp(&quot;'+esc(p.name)+'&quot;,this.checked)"></td>'
  +'<td>'+(p.status.running
    ?'<button onclick="act(&quot;'+esc(p.name)+'&quot;,&quot;stop&quot;)">停止</button> <button onclick="act(&quot;'+esc(p.name)+'&quot;,&quot;restart&quot;)">重启</button>'
    :'<button class="p" onclick="act(&quot;'+esc(p.name)+'&quot;,&quot;start&quot;)">启动</button>')+'</td></tr>';}
 document.getElementById('plugsbox').innerHTML=h+'</table>';}
async function act(n,a){try{await api('/api/plugins/'+n+'/'+a,{method:'POST'});}catch(e){alert(e.message);}loadPlugins();}
async function kp(n,k){try{await api('/api/plugins/'+n+'/set-keepalive',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({keepalive:k})});}catch(e){alert(e.message);}}
async function chpw(){const m=document.getElementById('msg');m.textContent='';
 const np=document.getElementById('np').value;
 if(np!==document.getElementById('np2').value){m.textContent='两次密码不一致';return;}
 try{await api('/api/account/password',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({old_password:document.getElementById('op').value,new_password:np})});
  m.style.color='#4a6b45';m.textContent='密码已修改';document.getElementById('op').value=document.getElementById('np').value=document.getElementById('np2').value='';}
 catch(e){m.style.color='#c46a5a';m.textContent=e.message;}}
async function logout(){try{await api('/api/logout',{method:'POST'});}catch(e){}location.href='/';}
function esc(s){return String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));}
boot();
</script></body></html>"#;

// ---------- 插件管理 ----------

#[derive(Deserialize, Clone)]
struct Manifest {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    bind: Option<String>,
    command: String,
    #[serde(default)]
    keepalive: Option<bool>,
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    menus: Option<Vec<MenuItem>>,
}

#[derive(Deserialize, Clone)]
struct MenuItem {
    title: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    section: Option<String>,
}

struct Runtime {
    port: u16,
    pid: u32,
    last_touch: std::time::Instant,
    keepalive: bool,
}

#[derive(Clone)]
struct App {
    home: Arc<String>,
    port_lo: u16,
    port_hi: u16,
    runtimes: Arc<Mutex<HashMap<String, Runtime>>>,
}

fn load_manifest(home: &str, name: &str) -> Option<Manifest> {
    let p = PathBuf::from(home).join("plugins").join(name).join("manifest.yaml");
    let data = fs::read_to_string(p).ok()?;
    serde_yaml::from_str(&data).ok()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !name.contains("..")
}

fn port_free(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_err()
}

fn wait_port(port: u16, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn save_port_map(home: &str, runtimes: &HashMap<String, Runtime>) {
    let mut m: HashMap<String, serde_json::Value> = HashMap::new();
    for (name, rt) in runtimes {
        m.insert(
            name.clone(),
            serde_json::json!({"port": rt.port, "pid": rt.pid, "started_at": now_secs().to_string()}),
        );
    }
    let p = PathBuf::from(home).join("etc").join("port-map.json");
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(p, serde_json::to_string_pretty(&m).unwrap_or_default());
}

fn spawn_plugin(app: &App, name: &str) -> Result<Runtime, String> {
    let mf =
        load_manifest(&app.home, name).ok_or_else(|| format!("插件 {} 的 manifest 无效", name))?;
    {
        let mut map = app.runtimes.lock().unwrap();
        if let Some(rt) = map.get_mut(name) {
            rt.last_touch = std::time::Instant::now();
            return Ok(Runtime { port: rt.port, pid: rt.pid, last_touch: rt.last_touch, keepalive: rt.keepalive });
        }
    }
    let port = (app.port_lo..=app.port_hi)
        .find(|p| port_free(*p))
        .ok_or("插件端口池已耗尽")?;
    let bind = mf.bind.clone().unwrap_or_else(|| "127.0.0.1".to_string());
    let cmd_path = PathBuf::from(&*app.home).join("plugins").join(name).join(&mf.command);
    let log_dir = PathBuf::from(&*app.home).join("logs").join("plugins");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(format!("{}.log", name));
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("打开日志失败: {}", e))?;
    let mut child = Command::new(&cmd_path)
        .env("PLUGIN_PORT", port.to_string())
        .env("PLUGIN_BIND", &bind)
        .env("PLUGIN_NAME", name)
        .env("PANEL_HOME", &*app.home)
        .env("IOTAPANEL_VERSION", "0.4.1")
        .stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .map_err(|e| format!("启动插件失败: {}", e))?;
    let pid = child.id();
    if !wait_port(port, Duration::from_secs(6)) {
        let _ = child.kill();
        return Err(format!("插件 {} 启动超时（6s）", name));
    }
    let keepalive = plugin_keepalive(app, name);
    app.runtimes.lock().unwrap().insert(
        name.to_string(),
        Runtime { port, pid, last_touch: std::time::Instant::now(), keepalive },
    );
    save_port_map(&app.home, &app.runtimes.lock().unwrap());
    Ok(Runtime { port, pid, last_touch: std::time::Instant::now(), keepalive })
}

fn plugin_keepalive(app: &App, name: &str) -> bool {
    let panel = load_panel(&app.home);
    for p in &panel.plugins {
        if p.get("name").and_then(|x| x.as_str()) == Some(name) {
            if let Some(k) = p.get("keepalive").and_then(|x| x.as_bool()) {
                return k;
            }
        }
    }
    load_manifest(&app.home, name)
        .map(|m| m.keepalive.unwrap_or(false))
        .unwrap_or(false)
}

fn set_plugin_keepalive(app: &App, name: &str, keepalive: bool) {
    let mut panel = load_panel(&app.home);
    let mut found = false;
    for p in panel.plugins.iter_mut() {
        if p.get("name").and_then(|x| x.as_str()) == Some(name) {
            if let Some(obj) = p.as_object_mut() {
                obj.insert("keepalive".into(), serde_json::json!(keepalive));
            }
            found = true;
        }
    }
    if !found {
        panel.plugins.push(serde_json::json!({"name": name, "keepalive": keepalive}));
    }
    save_panel(&app.home, &panel);
}

fn plugin_status(app: &App) -> Vec<serde_json::Value> {
    let map = app.runtimes.lock().unwrap();
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(PathBuf::from(&*app.home).join("plugins")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !PathBuf::from(&*app.home)
                .join("plugins").join(&name).join("manifest.yaml").exists()
            {
                continue;
            }
            let mf = load_manifest(&app.home, &name);
            let running = map.contains_key(&name);
            let (pid, port) = match map.get(&name) {
                Some(rt) => (rt.pid as i64, rt.port as i64),
                None => (0, 0),
            };
            let menus: Vec<serde_json::Value> = mf
                .as_ref()
                .and_then(|m| m.menus.clone())
                .unwrap_or_default()
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "title": m.title,
                        "icon": m.icon,
                        "path": m.path.clone().unwrap_or_else(|| "/".to_string()),
                        "section": m.section,
                    })
                })
                .collect();
            out.push(serde_json::json!({
                "name": name,
                "title": mf.as_ref().and_then(|m| m.title.clone()).unwrap_or(name.clone()),
                "version": "0.0.0",
                "author": "",
                "description": "",
                "language": "",
                "keepalive": plugin_keepalive(app, &name),
                "menus": menus,
                "status": {"running": running, "pid": pid, "port": port},
            }));
        }
    }
    out
}

async fn handle_plugins_api(app: App, path: String, req: Request<hyper::body::Incoming>) -> Response<Body> {
    let secret = secret_from_env(&app.home);
    if !authed(&app.home, req.headers(), &secret) {
        return unauthorized();
    }
    let rest = path.trim_start_matches("/api/plugins").trim_start_matches('/');
    let parts: Vec<&str> = rest.split('/').filter(|x| !x.is_empty()).collect();
    match parts.as_slice() {
        [name, _] if !valid_name(name) => {
            return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"非法插件名\"}");
        }
        [] => json_resp(
            StatusCode::OK,
            &serde_json::to_string(&serde_json::json!({"plugins": plugin_status(&app)}))
                .unwrap_or_default(),
        ),
        [name, action] => match *action {
            "start" => match spawn_plugin(&app, name) {
                Ok(r) => json_resp(
                    StatusCode::OK,
                    &format!("{{\"ok\":true,\"pid\":{},\"port\":{}}}", r.pid, r.port),
                ),
                Err(e) => json_resp(StatusCode::BAD_REQUEST, &format!("{{\"error\":\"{}\"}}", e)),
            },
            "stop" => {
                let pid = app.runtimes.lock().unwrap().remove(*name).map(|r| r.pid);
                if let Some(pid) = pid {
                    let _ = std::process::Command::new("kill")
                        .arg("-TERM").arg(pid.to_string()).status();
                    save_port_map(&app.home, &app.runtimes.lock().unwrap());
                }
                json_resp(StatusCode::OK, "{\"ok\":true}")
            }
            "restart" => {
                let pid = app.runtimes.lock().unwrap().remove(*name).map(|r| r.pid);
                if let Some(pid) = pid {
                    let _ = std::process::Command::new("kill")
                        .arg("-TERM").arg(pid.to_string()).status();
                    save_port_map(&app.home, &app.runtimes.lock().unwrap());
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                match spawn_plugin(&app, name) {
                    Ok(r) => json_resp(
                        StatusCode::OK,
                        &format!("{{\"ok\":true,\"pid\":{},\"port\":{}}}", r.pid, r.port),
                    ),
                    Err(e) => json_resp(StatusCode::BAD_REQUEST, &format!("{{\"error\":\"{}\"}}", e)),
                }
            }
            "set-keepalive" => {
                let body = match req.into_body().collect().await {
                    Ok(b) => String::from_utf8_lossy(&b.to_bytes()).to_string(),
                    Err(_) => return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"读取失败\"}"),
                };
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                let k = v.get("keepalive").and_then(|x| x.as_bool()).unwrap_or(false);
                set_plugin_keepalive(&app, name, k);
                json_resp(StatusCode::OK, "{\"ok\":true}")
            }
            _ => json_resp(StatusCode::NOT_FOUND, "{\"error\":\"未知操作\"}"),
        },
        _ => json_resp(StatusCode::NOT_FOUND, "{\"error\":\"not found\"}"),
    }
}

// 空闲执行器：非保活插件空闲超时后退出；保活插件不在运行则拉起
fn idle_sweep(app: &App) {
    let timeout = env::var("IDLE_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);
    let mut map = app.runtimes.lock().unwrap();
    let mut dead = Vec::new();
    for (name, rt) in map.iter() {
        if !rt.keepalive && rt.last_touch.elapsed() > Duration::from_secs(timeout) {
            dead.push((name.clone(), rt.pid));
        }
    }
    if !dead.is_empty() {
        for (name, pid) in &dead {
            let _ = std::process::Command::new("kill")
                .arg("-TERM").arg(pid.to_string()).status();
            map.remove(name);
            println!("[iotapanel-rust] 空闲退出: {}", name);
        }
        save_port_map(&app.home, &map);
    }
    // 保活插件自愈：不在运行则重新拉起
    if let Ok(entries) = fs::read_dir(PathBuf::from(&*app.home).join("plugins")) {
        for e in entries.flatten() {
            let nm = e.file_name().to_string_lossy().to_string();
            let kp = load_manifest(&app.home, &nm)
                .map(|m| m.keepalive.unwrap_or(false))
                .unwrap_or(false);
            if kp && !map.contains_key(&nm) {
                drop(map);
                if spawn_plugin(app, &nm).is_ok() {
                    println!("[iotapanel-rust] 保活插件已拉起: {}", nm);
                }
                map = app.runtimes.lock().unwrap();
            }
        }
    }
}

async fn gateway(app: App, path: String, req: Request<hyper::body::Incoming>) -> Response<Body> {
    let secret = secret_from_env(&app.home);
    let rest_path = path.strip_prefix("/p/").unwrap_or(&path);
    let (name, rest) = match rest_path.split_once('/') {
        Some((n, r)) => (n.to_string(), r.to_string()),
        None => (rest_path.to_string(), String::new()),
    };
    if !valid_name(&name) {
        return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"非法插件名\"}");
    }
    // auth: none 且路径为 /mcp → 免面板登录（插件自带鉴权，如 MCP Agent）
    let exempt = load_manifest(&app.home, &name)
        .map(|m| m.auth.as_deref() == Some("none") && rest == "mcp")
        .unwrap_or(false);
    if !exempt && !authed(&app.home, req.headers(), &secret) {
        return unauthorized();
    }
    let rt = match spawn_plugin(&app, &name) {
        Ok(r) => r,
        Err(e) => return json_resp(StatusCode::BAD_GATEWAY, &format!("{{\"error\":\"{}\"}}", e)),
    };
    // 请求即 touch：刷新空闲计时
    if let Some(rt) = app.runtimes.lock().unwrap().get_mut(&name) {
        rt.last_touch = std::time::Instant::now();
    }
    let fwd_path = if rest.starts_with('/') { rest.clone() } else { format!("/{}", rest) };
    let uri = format!("http://127.0.0.1:{}{}", rt.port, fwd_path);
    let client: HyperClient<HttpConnector, Body> =
        HyperClient::builder(TokioExecutor::new()).build(HttpConnector::new());
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return json_resp(StatusCode::BAD_GATEWAY, "{\"error\":\"读取请求失败\"}"),
    };
    let mut builder = hyper::Request::builder().method(method).uri(uri);
    for (k, v) in headers.iter() {
        if k.as_str() != "host" {
            builder = builder.header(k, v);
        }
    }
    let fwd_req = builder.body(Body::from(body_bytes)).unwrap();
    // per-request 超时：插件挂起不再让请求永久悬挂（PROXY_TIMEOUT 秒，默认 30）
    let fwd_fut = async {
        let resp = client
            .request(fwd_req)
            .await
            .map_err(|e| format!("插件连接失败: {}", e))?;
        let status = resp.status();
        let hdrs = resp.headers().clone();
        let body = resp
            .into_body()
            .collect()
            .await
            .map_err(|_| "读取响应失败".to_string())?
            .to_bytes()
            .to_vec();
        Ok::<_, String>((status, hdrs, body))
    };
    let proxy_timeout = env::var("PROXY_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    match tokio::time::timeout(Duration::from_secs(proxy_timeout), fwd_fut).await {
        Ok(Ok((status, hdrs, body))) => {
            let mut rb = Response::builder().status(status);
            for (k, v) in hdrs.iter() {
                rb = rb.header(k, v);
            }
            rb.body(Body::from(body)).unwrap()
        }
        Ok(Err(e)) => json_resp(StatusCode::BAD_GATEWAY, &format!("{{\"error\":\"{}\"}}", e)),
        Err(_) => json_resp(StatusCode::BAD_GATEWAY, "{\"error\":\"插件响应超时\"}"),
    }
}

async fn dispatch(app: App, req: Request<hyper::body::Incoming>) -> Response<Body> {
    let path = req.uri().path().to_string();
    let secret = secret_from_env(&app.home);
    match path.as_str() {
        "/api/login" => {
            let hdrs = req.headers().clone();
            let body = match req.into_body().collect().await {
                Ok(b) => String::from_utf8_lossy(&b.to_bytes()).to_string(),
                Err(_) => return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"读取失败\"}"),
            };
            handle_login(&app, &hdrs, &body)
        }
        "/api/logout" => handle_logout(&app, req.headers()),
        "/api/setup/state" => {
            handle_setup_state(&app.home)
        }
        "/api/setup/start" => {
            let hdrs = req.headers().clone();
            let body = match req.into_body().collect().await {
                Ok(b) => String::from_utf8_lossy(&b.to_bytes()).to_string(),
                Err(_) => return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"读取失败\"}"),
            };
            if let Some(origin) = hdrs.get("origin").and_then(|v| v.to_str().ok()) {
                let host = hdrs.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
                if !origin_ok(origin, host) {
                    return json_resp(StatusCode::FORBIDDEN, "{\"error\":\"跨站请求被拒绝\"}");
                }
            }
            handle_setup_start(&app, &body)
        }
        "/api/me" => {
            let secret2 = secret_from_env(&app.home);
            let token = match cookie_value(req.headers(), COOKIE) {
                Some(t) => t,
                None => return unauthorized(),
            };
            let user = match verify_token(&token, &secret2) {
                Some((_j, u)) => u,
                None => return unauthorized(),
            };
            let th = sha256_hex(&token);
            let ok = load_panel(&app.home).sessions.iter().any(|s| {
                s.token_hash == th && !s.revoked && s.username == user
            });
            if !ok {
                return unauthorized();
            }
            json_resp(StatusCode::OK, &format!("{{\"username\":\"{}\"}}", user))
        }
        "/api/overview" => {
            let secret2 = secret_from_env(&app.home);
            if !authed(&app.home, req.headers(), &secret2) {
                return unauthorized();
            }
            let np = plugin_status(&app).len();
            let nr = app.runtimes.lock().unwrap().len();
            json_resp(
                StatusCode::OK,
                &format!(
                    "{{\"version\":\"0.4.1\",\"home\":\"{}\",\"plugins\":{},\"running\":{}}}",
                    app.home.replace('\\', "/"), np, nr
                ),
            )
        }
        "/api/account/password" => {
            let hdrs = req.headers().clone();
            let body = match req.into_body().collect().await {
                Ok(b) => String::from_utf8_lossy(&b.to_bytes()).to_string(),
                Err(_) => return json_resp(StatusCode::BAD_REQUEST, "{\"error\":\"读取失败\"}"),
            };
            handle_password(&app, &hdrs, &body)
        }
        "/setup" | "/setup/" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Body::from(SETUP_HTML))
            .unwrap(),
        "/login" | "/login/" => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Body::from(LOGIN_HTML))
            .unwrap(),
        "/" => {
            if !configured(&app.home) {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header("Location", "/setup")
                    .body(Body::empty())
                    .unwrap()
            } else if authed(&app.home, req.headers(), &secret) {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .body(Body::from(ADMIN_HTML))
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header("Location", "/login")
                    .body(Body::empty())
                    .unwrap()
            }
        }
        p if p.starts_with("/api/plugins") => handle_plugins_api(app, path, req).await,
        p if p.starts_with("/p/") => gateway(app, path, req).await,
        _ => json_resp(StatusCode::NOT_FOUND, "{\"error\":\"not found\"}"),
    }
}

#[tokio::main]
async fn main() {
    let home = env::var("PANEL_HOME").unwrap_or_else(|_| "/data/panel".to_string());
    let listen = env::var("LISTEN_ADDR").unwrap_or_else(|_| ":8787".to_string());
    let app = App {
        home: Arc::new(home.clone()),
        port_lo: env::var("PORT_START").ok().and_then(|v| v.parse().ok()).unwrap_or(19000),
        port_hi: env::var("PORT_END").ok().and_then(|v| v.parse().ok()).unwrap_or(19999),
        runtimes: Arc::new(Mutex::new(HashMap::new())),
    };
    println!("[iotapanel-rust v0.2] home={} listen={}", home, listen);
    // 空闲执行 + 保活自愈（每 5 秒）
    {
        let sweep_app = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                idle_sweep(&sweep_app);
            }
        });
    }
    // 启动时的保活插件恢复
    {
        let revive_app = app.clone();
        tokio::spawn(async move {
            if let Ok(entries) = fs::read_dir(PathBuf::from(&*revive_app.home).join("plugins")) {
                let mut names = Vec::new();
                for e in entries.flatten() {
                    let nm = e.file_name().to_string_lossy().to_string();
                    if load_manifest(&revive_app.home, &nm)
                        .map(|m| m.keepalive.unwrap_or(false))
                        .unwrap_or(false)
                    {
                        names.push(nm);
                    }
                }
                for n in names {
                    if spawn_plugin(&revive_app, &n).is_ok() {
                        println!("[iotapanel-rust] 保活插件已启动: {}", n);
                    }
                }
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(&listen).await.expect("绑定监听失败");
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let app = app.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req: Request<hyper::body::Incoming>| {
                let app = app.clone();
                async move { Ok::<_, hyper::Error>(dispatch(app, req).await) }
            });
            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
        });
    }
}