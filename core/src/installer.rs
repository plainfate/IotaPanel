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

//! 插件安装：内嵌官方包释放 + 远程 URL 包下载安装。
//! 包布局要求与 Go 版一致：tar.gz 内恰好一个顶层目录，内含 manifest.yaml。

use crate::embed;
use crate::manifest::Manifest;
use std::io::Read;
use std::path::Path;

/// 内嵌目录中是否存在该插件。
pub fn catalog_contains(name: &str) -> bool {
    embed::file(&format!("plugins/{}/manifest.yaml", name)).is_some()
}

/// 商城条目。
pub struct CatalogItem {
    pub name: String,
    pub title: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub language: String,
    pub installed: bool,
}

pub fn list_catalog() -> Vec<CatalogItem> {
    embed::list_dir("plugins")
        .into_iter()
        .filter_map(|name| {
            let data = embed::file(&format!("plugins/{}/manifest.yaml", name))?;
            let y = iotapanel_sdk::util::parse_yaml(&String::from_utf8_lossy(data));
            Some(CatalogItem {
                name: y.str_or("name", &name),
                title: y.str_or("title", &name),
                version: y.str_or("version", ""),
                author: y.str_or("author", ""),
                description: y.str_or("description", ""),
                language: y.str_or("language", ""),
                installed: false,
            })
        })
        .collect()
}

/// 把内嵌插件包完整复制到 PANEL_HOME/plugins/<name>；bin/*.gz 自动解压并赋可执行权限。
pub fn install_from_embed(home: &str, name: &str) -> Result<(), String> {
    let prefix = format!("plugins/{}", name);
    if !catalog_contains(name) {
        return Err(format!("插件包不存在: {}", name));
    }
    let dest = Path::new(home).join("plugins").join(name);
    // 保留 install.sh 或上一次安装放置的 bin/，只更新内嵌配置与静态资源。
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for p in embed::all_paths() {
        let rel = match p.strip_prefix(&prefix) {
            Some(r) => r.trim_start_matches('/'),
            None => continue,
        };
        if rel.is_empty() {
            continue;
        }
        // 跳过 web/ 静态资源以外的目录写盘？—— 与 Go 版一致全部落盘（插件页面自带）
        let target = dest.join(rel);
        if let Some(dirp) = target.parent() {
            std::fs::create_dir_all(dirp).map_err(|e| e.to_string())?;
        }
        let data = embed::file(p).ok_or("内嵌资源读取失败")?;
        if rel.starts_with("bin/") && p.ends_with(".gz") {
            let raw = gunzip(data)?;
            let final_path = target.with_extension(""); // 去掉 .gz
            std::fs::write(&final_path, &raw).map_err(|e| e.to_string())?;
            set_executable(&final_path);
        } else {
            std::fs::write(&target, data).map_err(|e| e.to_string())?;
            if rel.starts_with("bin/") {
                set_executable(&target);
            }
        }
    }
    Ok(())
}

/// 解压远程 tar.gz 插件包到临时校验结构：
/// 返回 (顶层目录名, 相对路径 → 内容)。防路径穿越 / 多顶层目录 / gzip 炸弹。
pub fn unpack_plugin_package(data: &[u8]) -> Result<(String, Vec<(String, Vec<u8>)>), String> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    let mut top_dir = String::new();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let total_cap = (64 << 20) as usize;
    let mut total = 0usize;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
        let clean = clean_rel_path(&path)?;
        let parts: Vec<&str> = clean.splitn(2, '/').collect();
        if parts.len() < 2 || parts[1].is_empty() {
            continue; // 顶层文件忽略
        }
        if top_dir.is_empty() {
            top_dir = parts[0].to_string();
        } else if top_dir != parts[0] {
            return Err("包内存在多个顶层目录".into());
        }
        let mut content = Vec::new();
        let mut limited = entry.take((total_cap - total) as u64);
        limited.read_to_end(&mut content).map_err(|e| e.to_string())?;
        total += content.len();
        if total > total_cap {
            return Err("插件包解压后总大小超过上限（256MB）".into());
        }
        files.push((parts[1].to_string(), content));
    }
    if top_dir.is_empty() {
        return Err("包内未找到插件目录".into());
    }
    if !files.iter().any(|(r, _)| r == "manifest.yaml") {
        return Err("插件目录缺少 manifest.yaml".into());
    }
    Ok((top_dir, files))
}

/// 从 URL 安装：下载（64MB 封顶）→ 可选 SHA256 校验 → 解压 → 落盘 → 返回 manifest。
pub struct RemoteInstall {
    pub manifest: Manifest,
    pub name: String,
}

pub fn install_from_url(
    home: &str,
    url: &str,
    expected_sha256: &str,
) -> Result<RemoteInstall, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("仅支持 http/https 下载地址".into());
    }
    let data = http_get(url, 60, 64 << 20)?;
    if !expected_sha256.trim().is_empty() {
        let sum = crate::util::sha256_hex(&data);
        if sum != expected_sha256.trim().to_ascii_lowercase() {
            return Err("SHA256 校验失败，包可能被篡改或下载不完整".into());
        }
    }
    let (name, files) = unpack_plugin_package(&data)
        .map_err(|e| format!("插件包解析失败: {}", e))?;

    let dest = Path::new(home).join("plugins").join(&name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    for (rel, content) in &files {
        let target = dest.join(rel);
        if let Some(dirp) = target.parent() {
            std::fs::create_dir_all(dirp).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, content).map_err(|e| e.to_string())?;
        if rel.starts_with("bin/") {
            set_executable(&target);
        }
    }
    let mf = Manifest::load(&dest).map_err(|e| format!("manifest 解析失败: {}", e))?;
    Ok(RemoteInstall { manifest: mf, name })
}

// ---------- 底层 ----------

fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|e| format!("解压插件失败: {}", e))?;
    Ok(out)
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(path) {
        let mut perm = md.permissions();
        perm.set_mode(0o755);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn set_executable(_path: &PathBuf) {}

/// 规整 tar 成员路径并拒绝穿越。
fn clean_rel_path(p: &Path) -> Result<String, String> {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut parts = Vec::new();
    for seg in s.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(format!("非法路径: {}", s)),
            other => parts.push(other),
        }
    }
    Ok(parts.join("/"))
}

/// 极简阻塞 HTTP 客户端（GET），够下载插件包用；支持 https 时由调用方保证平台 TLS。
/// 这里只实现 http:// —— 插件生态约定 GitHub Release 直链（https），
/// 因此 https 通过系统 curl 子进程兜底，保持零 TLS 依赖。
pub fn http_get(url: &str, timeout_secs: u64, max_size: usize) -> Result<Vec<u8>, String> {
    if url.starts_with("https://") {
        // 走系统 curl（绝大多数发行版预装）；参数收紧：跟随重定向、限速防护
        let out = std::process::Command::new("curl")
            .args([
                "-fsSL",
                "--max-time",
                &timeout_secs.to_string(),
                "--max-filesize",
                &max_size.to_string(),
                url,
            ])
            .output()
            .map_err(|e| format!("下载失败: {}", e))?;
        if !out.status.success() {
            return Err(format!("下载失败: curl 退出码 {:?}", out.status.code()));
        }
        if out.stdout.len() > max_size {
            return Err("下载内容超过大小上限".into());
        }
        return Ok(out.stdout);
    }
    // http:// 用内置客户端
    let rest = url.strip_prefix("http://").unwrap();
    let (hostport, path) = match rest.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{}", p)),
        None => (rest.to_string(), "/".to_string()),
    };
    use std::io::{Read, Write};
    use std::net::TcpStream;
    let addr = if hostport.contains(':') { hostport.clone() } else { format!("{}:80", hostport) };
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("下载失败: {}", e))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(timeout_secs)))
        .ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: IotaPanel/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path,
        hostport,
        crate::config::VERSION
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("下载失败: {}", e))?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| format!("读取下载内容失败: {}", e))?;
    // 切状态行 / 头 / 体（Connection: close，读到 EOF 即完整 body）
    let sep = window_find(&buf, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..sep]).to_string();
    let status_line = head.lines().next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    if code != 200 {
        return Err(format!("下载失败: HTTP {}", code));
    }
    let body = buf[sep + 4..].to_vec();
    if body.len() > max_size {
        return Err("下载内容超过大小上限".into());
    }
    Ok(body)
}

fn window_find(haystack: &[u8], needle: &[u8]) -> Result<usize, String> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .ok_or_else(|| "响应格式错误".to_string())
}
