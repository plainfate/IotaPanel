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

//! 文件管理插件（纯 Rust 重写）。
//! 浏览/上传/下载/删除/重命名服务器文件。经面板网关 /p/file-manager/ 访问。

use iotapanel_sdk::http::{Request, Response};
use iotapanel_sdk::util;
use std::path::{Path, PathBuf};

const INDEX_HTML: &str = include_str!("../web/index.html");

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("PLUGIN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(19003);

    let handler = |req: &Request| handle(req);
    eprintln!("[file-manager] listening on {}:{}", bind, port);
    if let Err(e) = iotapanel_sdk::http::serve(&bind, port, handler) {
        eprintln!("[file-manager] server error: {}", e);
        std::process::exit(1);
    }
}

fn handle(req: &Request) -> Response {
    if req.method == "GET" || req.method == "HEAD" {
        if req.path.starts_with("/api/") {
            return match req.path.as_str() {
                "/api/list" => api_list(req),
                "/api/download" => api_download(req),
                "/api/read" => api_read_query(req),
                "/api/disks" => api_disks(),
                _ => Response::json_err(404, "not found"),
            };
        }
        let mut r = Response::html(INDEX_HTML);
        r.headers.push(("Cache-Control".into(), "no-cache".into()));
        return r;
    }
    // POST /api/upload 走 multipart 解析；其余 POST 才要求 JSON 体
    if req.path == "/api/upload" {
        return api_upload_multipart(req);
    }
    let body: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => return Response::json_err(400, "请求体不是有效 JSON"),
    };
    match req.path.as_str() {
        "/api/read" => api_read(&body),
        "/api/write" => api_write(&body),
        "/api/mkdir" => api_mkdir(&body),
        "/api/delete" => api_delete(&body),
        "/api/rename" => api_rename(&body),
        "/api/upload-base64" => api_upload_base64(&body),
        _ => Response::json_err(404, "not found"),
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> Result<String, Response> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| Response::json_err(400, &format!("缺少字段 {}", key)))
}

/// 规范化绝对路径，阻止 NUL 与空路径。
fn safe_path(input: &str) -> Result<PathBuf, Response> {
    if input.is_empty() || input.contains('\0') {
        return Err(Response::json_err(400, "非法路径"));
    }
    let p = Path::new(input);
    // 相对路径基于当前工作目录
    Ok(if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(p) })
}

fn api_list(req: &Request) -> Response {
    let q = req.query();
    let raw = q.get("path").cloned().filter(|s| !s.is_empty()).unwrap_or_default();
    let dir = match safe_path(&raw) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dir = if raw.is_empty() {
        std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
    } else {
        dir
    };
    let rd = match std::fs::read_dir(&dir) {
        Ok(d) => d,
        Err(e) => return Response::json_err(404, &format!("无法读取目录: {}", e)),
    };
    let mut entries = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let full = e.path();
        let (is_dir, size, mtime, perm) = match std::fs::symlink_metadata(&full) {
            Ok(md) => {
                let isln = md.file_type().is_symlink();
                let (is_dir, size, mtime) = if isln {
                    // 解析链接目标
                    match std::fs::metadata(&full) {
                        Ok(t) => (t.is_dir(), t.len(), t.modified().ok().map(|m| m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)).unwrap_or(0)),
                        Err(_) => (false, 0, 0),
                    }
                } else {
                    (md.is_dir(), md.len(), md.modified().ok().map(|m| m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)).unwrap_or(0))
                };
                (is_dir, size, mtime, unix_perm(&md))
            }
            Err(_) => (false, 0, 0, String::new()),
        };
        // 字段以 web/index.html 前端契约为准：dir / mode / owner / size / mtime(格式化字符串)
        // 同时保留 is_dir / permission 以兼容旧调用方。
        let owner = owner_of(&full);
        entries.push(serde_json::json!({
            "name": name,
            "path": full.to_string_lossy(),
            "is_dir": is_dir,
            "dir": is_dir,
            "size": size,
            "mode": perm,
            "permission": perm,
            "owner": owner,
            "mtime": util::human_mtime(mtime as i64),
        }));
    }
    entries.sort_by(|a, b| {
        let ad = a["is_dir"].as_bool().unwrap_or(false);
        let bd = b["is_dir"].as_bool().unwrap_or(false);
        bd.cmp(&ad).then_with(|| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")))
    });
    Response::json(&serde_json::json!({
        "path": dir.to_string_lossy(),
        "entries": entries,
    }))
}

fn unix_perm(md: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = md.permissions().mode();
        let mut s = String::new();
        s.push(if md.is_dir() { 'd' } else if md.file_type().is_symlink() { 'l' } else { '-' });
        for shift in [6, 3, 0] {
            let bits = (m >> shift) & 7;
            s.push(if bits & 4 != 0 { 'r' } else { '-' });
            s.push(if bits & 2 != 0 { 'w' } else { '-' });
            s.push(if bits & 1 != 0 { 'x' } else { '-' });
        }
        s
    }
    #[cfg(not(unix))]
    {
        let _ = md; String::new()
    }
}

/// 列出磁盘/挂载点（字段匹配 web/index.html loadDisks 契约：mountpoint/device/fstype/percent/used/total）。
fn api_disks() -> Response {
    #[cfg(unix)]
    {
        let mut rows = Vec::new();
        match std::fs::read_to_string("/proc/mounts") {
            Ok(s) => {
                let mut seen: Vec<String> = Vec::new();
                for line in s.lines() {
                    let f: Vec<&str> = line.split_whitespace().collect();
                    if f.len() < 3 {
                        continue;
                    }
                    let (device, mountpoint, fstype) = (f[0], f[1], f[2]);
                    // 忽略虚拟/伪文件系统，仅取真实磁盘挂载
                    if fstype.starts_with("proc")
                        || fstype.starts_with("sysfs")
                        || fstype.starts_with("cgroup")
                        || fstype == "devpts"
                        || fstype == "devtmpfs"
                        || fstype == "tmpfs"
                        || fstype == "overlay"
                        || fstype == "squashfs"
                        || fstype == "ramfs"
                        || fstype == "mqueue"
                        || fstype == "pstore"
                        || fstype == "securityfs"
                        || fstype == "debugfs"
                        || fstype == "tracefs"
                        || fstype == "configfs"
                        || fstype == "fusectl"
                        || device == "none"
                        || device.starts_with("shm")
                        // 伪挂载点（/sys、/proc 下）无实际磁盘，排除
                        || mountpoint.starts_with("/sys")
                        || mountpoint.starts_with("/proc")
                        || mountpoint.starts_with("/dev")
                    {
                        continue;
                    }
                    if seen.iter().any(|x| x == mountpoint) {
                        continue;
                    }
                    seen.push(mountpoint.to_string());
                    // statvfs 容量
                    let (total, used, pct) = mount_usage(mountpoint);
                    rows.push(serde_json::json!({
                        "mountpoint": mountpoint,
                        "device": device,
                        "fstype": fstype,
                        "total": total,
                        "used": used,
                        "percent": pct,
                    }));
                }
            }
            Err(_) => {}
        }
        Response::json(&serde_json::json!({ "disks": rows }))
    }
    #[cfg(not(unix))]
    {
        Response::json(&serde_json::json!({ "disks": [] }))
    }
}

#[cfg(unix)]
fn mount_usage(mount: &str) -> (u64, u64, f64) {
    let c = std::ffi::CString::new(mount.as_bytes()).ok();
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if let Some(c) = c {
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return (0, 0, 0.0);
        }
    } else {
        return (0, 0, 0.0);
    }
    let bsize = st.f_frsize as u64;
    let total = st.f_blocks as u64 * bsize;
    let used = st.f_blocks as u64 * bsize - st.f_bfree as u64 * bsize;
    let pct = if total > 0 { used as f64 / total as f64 * 100.0 } else { 0.0 };
    (total, used, (pct * 10.0).round() / 10.0)
}

/// 解析文件/目录属主：uid -> /etc/passwd 用户名；解析失败则回退为 uid 字符串。
fn owner_of(path: &std::path::Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata(path).map(|m| m.uid()).unwrap_or(0);
        if let Ok(pw) = std::fs::read_to_string("/etc/passwd") {
            for line in pw.lines() {
                let f: Vec<&str> = line.split(':').collect();
                if f.len() >= 3 && f[2].trim() == uid.to_string() {
                    return f[0].to_string();
                }
            }
        }
        uid.to_string()
    }
    #[cfg(not(unix))]
    {
        let _ = path; String::new()
    }
}

fn api_download(req: &Request) -> Response {
    let q = req.query();
    let path = match q.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return Response::json_err(400, "缺少 path"),
    };
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let data = match std::fs::read(&p) {
        Ok(d) => d,
        Err(e) => return Response::json_err(404, &format!("读取失败: {}", e)),
    };
    let fname = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "download".into());
    let mut r = Response::new(200).with_body(data);
    r.headers.push(("Content-Type".into(), "application/octet-stream".into()));
    r.headers.push(("Content-Disposition".into(), format!("attachment; filename=\"{}\"", fname)));
    r
}

fn api_read(body: &serde_json::Value) -> Response {
    let path = match str_field(body, "path") {
        Ok(p) => p,
        Err(r) => return r,
    };
    read_file(&path)
}

/// GET /api/read?path=...（前端 viewFile 走此路径）
fn api_read_query(req: &Request) -> Response {
    let q = req.query();
    let path = match q.get("path").filter(|s| !s.is_empty()) {
        Some(p) => p.clone(),
        None => return Response::json_err(400, "缺少 path"),
    };
    read_file(&path)
}

/// 读取文本文件：返回 content/path/size；>2MB 拒绝。
fn read_file(path: &str) -> Response {
    let p = match safe_path(path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Ok(md) = std::fs::metadata(&p) {
        if md.len() > 2 * 1024 * 1024 {
            return Response::json_err(413, "文件过大（>2MB），请下载查看");
        }
    }
    match std::fs::read(&p) {
        Ok(data) => {
            let size = data.len();
            Response::json(&serde_json::json!({
                "content": String::from_utf8_lossy(&data),
                "path": p.to_string_lossy(),
                "size": size,
            }))
        }
        Err(e) => Response::json_err(500, &format!("读取失败: {}", e)),
    }
}

fn api_write(body: &serde_json::Value) -> Response {
    let path = match str_field(body, "path") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let content = body.get("content").and_then(|c| c.as_str()).unwrap_or("");
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(parent) = p.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Response::json_err(500, &format!("创建目录失败: {}", e));
        }
    }
    match std::fs::write(&p, content) {
        Ok(_) => Response::json(&serde_json::json!({"ok": true})),
        Err(e) => Response::json_err(500, &format!("写入失败: {}", e)),
    }
}

fn api_mkdir(body: &serde_json::Value) -> Response {
    let path = match str_field(body, "path") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match std::fs::create_dir_all(&p) {
        Ok(_) => Response::json(&serde_json::json!({"ok": true})),
        Err(e) => Response::json_err(500, &format!("创建失败: {}", e)),
    }
}

fn api_delete(body: &serde_json::Value) -> Response {
    let path = match str_field(body, "path") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (is_dir, exists) = match std::fs::symlink_metadata(&p) {
        Ok(md) => (md.is_dir(), true),
        Err(_) => (false, false),
    };
    if !exists {
        return Response::json_err(404, "路径不存在");
    }
    let result = if is_dir {
        std::fs::remove_dir_all(&p)
    } else {
        std::fs::remove_file(&p)
    };
    match result {
        Ok(_) => Response::json(&serde_json::json!({"ok": true})),
        Err(e) => Response::json_err(500, &format!("删除失败: {}", e)),
    }
}

fn api_rename(body: &serde_json::Value) -> Response {
    let from = match str_field(body, "path").or_else(|_| str_field(body, "from")) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let to = match str_field(body, "newname")
        .or_else(|_| str_field(body, "new_name"))
        .or_else(|_| str_field(body, "to")) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let src = match safe_path(&from) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dst = match safe_path(&to) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match std::fs::rename(&src, &dst) {
        Ok(_) => Response::json(&serde_json::json!({"ok": true})),
        Err(e) => Response::json_err(500, &format!("重命名失败: {}", e)),
    }
}

fn api_upload_base64(body: &serde_json::Value) -> Response {
    let path = match str_field(body, "path") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let b64 = match str_field(body, "base64") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let data = match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.as_bytes()) {
        Ok(d) => d,
        Err(e) => return Response::json_err(400, &format!("base64 解析失败: {}", e)),
    };
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&p, &data) {
        Ok(_) => Response::json(&serde_json::json!({"ok": true, "size": data.len()})),
        Err(e) => Response::json_err(500, &format!("保存失败: {}", e)),
    }
}

fn api_upload_multipart(req: &Request) -> Response {
    let ct = req.header("content-type").unwrap_or("");
    let Some(parts) = iotapanel_sdk::http::parse_multipart(ct, &req.body) else {
        return Response::json_err(400, "multipart 解析失败");
    };
    let mut path: Option<String> = None;
    let mut filedata: Option<Vec<u8>> = None;
    for part in parts {
        if part.name == "path" {
            path = Some(String::from_utf8_lossy(&part.data).into_owned());
        } else if part.name == "file" {
            filedata = Some(part.data);
        }
    }
    let (Some(path), Some(data)) = (path, filedata) else {
        return Response::json_err(400, "缺少 path 或 file 字段");
    };
    let p = match safe_path(&path) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&p, &data) {
        Ok(_) => Response::json(&serde_json::json!({"ok": true, "size": data.len()})),
        Err(e) => Response::json_err(500, &format!("保存失败: {}", e)),
    }
}
