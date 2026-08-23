//! Runtime configuration: environment variables + `etc/.env`.
//! Mirrors Go `internal/config`.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Panel core version (must match the original `config.Version`).
pub const VERSION: &str = "0.3.7";

#[derive(Clone, Debug)]
pub struct Config {
    pub home: String,
    pub listen_addr: String,
    pub jwt_secret: String,
    pub idle_timeout: Duration,
    pub port_lo: i32,
    pub port_hi: i32,
    pub trust_proxy: bool,
}

/// A small hook to let tests override working-directory-derived home.
static HOME_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

#[cfg(test)]
pub fn set_home_override(p: Option<String>) {
    *HOME_OVERRIDE.lock().unwrap() = p;
}

fn derive_home_from_executable() -> String {
    if let Ok(g) = HOME_OVERRIDE.lock() {
        if let Some(h) = g.as_ref() {
            return h.clone();
        }
    }
    // The core derives home from the executable location for non-systemd use.
    let exe = match env::current_exe() {
        Ok(e) => e,
        Err(_) => return String::new(),
    };
    let dir = match exe.parent() {
        Some(p) => p.to_string_lossy().to_string(),
        None => return String::new(),
    };
    let base = match Path::new(&dir).file_name() {
        Some(f) => f.to_string_lossy().to_string(),
        None => return String::new(),
    };
    if base != "bin" {
        return String::new();
    }
    let parent = match Path::new(&dir).parent() {
        Some(p) => p,
        None => return String::new(),
    };
    let ps = parent.to_string_lossy().to_string();
    if ps == "/" || ps == "." {
        return String::new();
    }
    ps
}

/// Parse a simple KEY=VALUE `.env` file and set env vars that are not already set.
fn load_env_file(path: &Path) {
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return,
    };
    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let k = line[..eq].trim();
            let mut v = line[eq + 1..].trim().to_string();
            if v.len() >= 2 && (v.starts_with('"') && v.ends_with('"') || v.starts_with('\'') && v.ends_with('\'')) {
                v = v[1..v.len() - 1].to_string();
            }
            if !k.is_empty() && env::var(k).unwrap_or_default().is_empty() {
                env::set_var(k, &v);
            }
        }
    }
}

/// Generate a crypto-random 32-byte hex secret.
fn generate_secret() -> std::io::Result<String> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(hex::encode(b))
}

/// Write/update `KEY=value` in the `.env` file (0600 perms).
fn save_env_var(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut lines: Vec<String> = Vec::new();
    if let Ok(data) = fs::read_to_string(path) {
        let mut found = false;
        for ln in data.lines() {
            let trimmed = ln.trim();
            if trimmed.starts_with(&format!("{}=", key)) {
                lines.push(format!("{}={}", key, value));
                found = true;
                continue;
            }
            lines.push(ln.to_string());
        }
        if !found {
            lines.push(format!("{}={}", key, value));
        }
    } else {
        lines.push(format!("{}={}", key, value));
    }
    let mut f = fs::OpenOptions::new().create(true).write(true).truncate(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    f.write_all((lines.join("\n") + "\n").as_bytes())?;
    Ok(())
}

/// Public setter used by the settings handler (mirrors `config.SetEnvVar`).
pub fn set_env_var(home: &str, key: &str, value: &str) -> std::io::Result<()> {
    save_env_var(&PathBuf::from(home).join("etc").join(".env"), key, value)
}

fn env_or(key: &str, def: &str) -> String {
    env::var(key).unwrap_or_else(|_| def.to_string())
}

pub fn load() -> Result<Config, String> {
    let mut home = env::var("PANEL_HOME").unwrap_or_default();
    if home.is_empty() {
        home = derive_home_from_executable();
    }
    let env_path = PathBuf::from(&home).join("etc").join(".env");
    load_env_file(&env_path);
    if let Ok(v) = env::var("PANEL_HOME") {
        if !v.is_empty() {
            home = v;
        }
    }
    if home.is_empty() {
        home = "/data/panel".to_string();
    }

    let mut cfg = Config {
        home,
        listen_addr: env_or("LISTEN_ADDR", ":8787"),
        jwt_secret: env::var("JWT_SECRET").unwrap_or_default(),
        idle_timeout: Duration::from_secs(5 * 60),
        port_lo: 19000,
        port_hi: 19999,
        trust_proxy: false,
    };

    if let Ok(v) = env::var("IDLE_TIMEOUT") {
        if let Ok(d) = parse_duration(&v) {
            if d > Duration::ZERO {
                cfg.idle_timeout = d;
            }
        }
    }
    if let Ok(v) = env::var("PORT_START") {
        if let Ok(n) = v.trim().parse::<i32>() {
            cfg.port_lo = n;
        }
    }
    if let Ok(v) = env::var("PORT_END") {
        if let Ok(n) = v.trim().parse::<i32>() {
            cfg.port_hi = n;
        }
    }
    if let Ok(v) = env::var("PANEL_TRUST_PROXY") {
        if v == "1" || v.eq_ignore_ascii_case("true") {
            cfg.trust_proxy = true;
        }
    }

    if cfg.jwt_secret.is_empty() {
        let secret = generate_secret().map_err(|e| e.to_string())?;
        cfg.jwt_secret = secret.clone();
        save_env_var(&env_path, "JWT_SECRET", &secret).map_err(|e| format!("写入 JWT_SECRET 失败: {}", e))?;
    }
    Ok(cfg)
}

/// Parse Go-style time.Duration strings like "5m", "30s", "2h30m".
fn parse_duration(s: &str) -> Result<Duration, String> {
    let mut total = Duration::ZERO;
    let mut num = String::new();
    let mut have_any = false;
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        if num.is_empty() {
            return Err("invalid duration".into());
        }
        let n: f64 = num.parse().map_err(|_| "invalid duration".to_string())?;
        num.clear();
        match c {
            'h' => add_unit(&mut total, n, 3600.0, &mut have_any)?,
            'm' | 'M' => add_unit(&mut total, n, 60.0, &mut have_any)?,
            's' => add_unit(&mut total, n, 1.0, &mut have_any)?,
            _ => return Err("invalid duration".into()),
        }
    }
    if !num.is_empty() {
        return Err("invalid duration".into());
    }
    if !have_any {
        return Err("invalid duration".into());
    }
    Ok(total)
}

fn add_unit(total: &mut Duration, n: f64, secs: f64, have_any: &mut bool) -> Result<(), String> {
    *have_any = true;
    let s = n * secs;
    if s < 0.0 {
        return Err("invalid duration".into());
    }
    *total += Duration::from_secs_f64(s);
    Ok(())
}