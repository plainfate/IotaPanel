//! Authentication: PBKDF2-SHA256 password hashing + HMAC-SHA256 signed
//! session cookie. Byte-compatible with the original Go `internal/auth`
//! (existing stored hashes and live sessions remain valid across a swap).

use base64::Engine;
use hmac::Mac;
use sha2::{Sha256, Digest};

pub const COOKIE_NAME: &str = "mp_session";
pub const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

const ITERATIONS: u32 = 600_000;
const LEGACY_ITERATIONS: u32 = 100_000;
const KEY_LEN: usize = 32;
const SALT_LEN: usize = 16;

#[derive(Debug, Clone)]
pub struct StoredPassword {
    pub salt: String,
    pub hash_hex: String,
}

/// Generate a new salt (`"iterations:hex"`) and PBKDF2-SHA256 hash (hex).
pub fn hash_password(pw: &str) -> std::io::Result<StoredPassword> {
    let mut salt_bytes = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt_bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut dk = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(pw.as_bytes(), &salt_bytes, ITERATIONS, &mut dk);
    Ok(StoredPassword {
        salt: format!("{}:{}", ITERATIONS, hex::encode(salt_bytes)),
        hash_hex: hex::encode(dk),
    })
}

/// Parse a salt: supports `"iterations:hex"` and legacy pure-hex (100k iters).
fn parse_salt(s: &str) -> Option<(u32, Vec<u8>)> {
    if let Some(i) = s.find(':') {
        if i > 0 {
            let iter: u32 = s[..i].parse().ok()?;
            if iter > 0 {
                let b = hex::decode(&s[i + 1..]).ok()?;
                return Some((iter, b));
            }
        }
    }
    let b = hex::decode(s).ok()?;
    Some((LEGACY_ITERATIONS, b))
}

/// Verify a password against a stored salt + hash hex (constant-time compare).
pub fn verify_password(pw: &str, salt: &str, hash_hex: &str) -> bool {
    let (iter, salt_bytes) = match parse_salt(salt) {
        Some(x) => x,
        None => return false,
    };
    let mut dk = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(pw.as_bytes(), &salt_bytes, iter, &mut dk);
    let got = hex::encode(dk);
    constant_time_eq(got.as_bytes(), hash_hex.as_bytes())
}

/// True if the stored salt is the legacy format (iterations differ from current),
/// i.e. the hash should be upgraded on next successful login.
pub fn needs_rehash(salt: &str) -> bool {
    match parse_salt(salt) {
        Some((iter, _)) => iter != ITERATIONS,
        None => false,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The signed session payload (mirrors Go `auth.Session`).
#[derive(Debug, Clone)]
pub struct Session {
    pub uid: i64,
    pub username: String,
    pub exp: i64,
    pub jti: String,
}

/// Generate a new session with a random JTI.
/// `ttl`: 24h for normal login, 30 days for "remember me".
pub fn new_session(uid: i64, username: &str, ttl: std::time::Duration) -> Session {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    Session {
        uid,
        username: username.to_string(),
        exp: chrono::Utc::now().timestamp() + ttl.as_secs() as i64,
        jti: hex::encode(b),
    }
}

/// Go-compatible JSON string encoding (HTML-escapes `<`, `>`, `&`, and
/// control chars the way `encoding/json` does by default).
fn go_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the token payload: `{"uid":N,"u":"..","exp":T,"j":".."}` (Go order).
fn build_payload(s: &Session) -> String {
    format!(
        "{{\"uid\":{},\"u\":{},\"exp\":{},\"j\":{}}}",
        s.uid,
        go_json_string(&s.username),
        s.exp,
        go_json_string(&s.jti)
    )
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Token: `payload.signature` (base64url, no padding).
pub fn token(s: &Session, secret: &[u8]) -> String {
    let payload = build_payload(s);
    let b = B64.encode(payload.as_bytes());
    format!("{}.{}", b, sign(&b, secret))
}

fn sign(data: &str, secret: &[u8]) -> String {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret).expect("hmac key");
    mac.update(data.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Validate signature + expiry and parse the payload.
pub fn parse_token(token: &str, secret: &[u8]) -> Option<Session> {
    let mut parts = token.splitn(2, '.');
    let b = parts.next()?;
    let sig = parts.next()?;
    let expected = sign(b, secret);
    if !constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        return None;
    }
    let data = B64.decode(b).ok()?;
    let s: serde_json::Value = serde_json::from_slice(&data).ok()?;
    let uid = s.get("uid")?.as_i64()?;
    let username = s.get("u")?.as_str()?.to_string();
    let exp = s.get("exp")?.as_i64()?;
    let jti = s.get("j")?.as_str()?.to_string();
    if exp < chrono::Utc::now().timestamp() {
        return None;
    }
    Some(Session { uid, username, exp, jti })
}

/// SHA-256 hex fingerprint of a token (used for the server-side session record).
pub fn sha256_hex(data: &str) -> String {
    hex::encode(Sha256::digest(data.as_bytes()))
}