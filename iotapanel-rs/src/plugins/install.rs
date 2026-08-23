//! Plugin catalog + installation from the embedded bundles, and unpacking of
//! remotely downloaded `.tar.gz` packages. Mirrors `internal/plugins/install.go`.

use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use serde::Serialize;

use crate::embed;
use crate::plugins::{load_manifest, Manifest};

const MAX_REMOTE_PLUGIN_SIZE: u64 = 64 << 20;

#[derive(Serialize, Debug, Clone)]
pub struct CatalogItem {
    pub name: String,
    pub title: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub language: String,
    pub installed: bool,
}

/// Recursively walk an embedded directory, invoking `f(path_rel, bytes)` for files.
fn walk_embed<F>(dir: &include_dir::Dir, is_bin: bool, f: &mut F) -> Result<(), String>
where
    F: FnMut(&str, Vec<u8>) -> Result<(), String>,
{
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(d) => {
                walk_embed(d, is_bin, f)?;
            }
            include_dir::DirEntry::File(file) => {
                let rel = file.path().to_string_lossy().to_string();
                let mut bytes = file.contents().to_vec();
                // bundled binaries are stored gzipped; decompress when the path
                // sits under a `bin/` dir and ends in `.gz` (mirrors build.sh)
                if is_bin && rel.contains("/bin/") && rel.ends_with(".gz") {
                    let mut out = Vec::new();
                    flate2::read::GzDecoder::new(&bytes[..])
                        .read_to_end(&mut out)
                        .map_err(|e| format!("解压插件失败: {}", e))?;
                    bytes = out;
                    let rel_without = rel.trim_end_matches(".gz");
                    f(rel_without, bytes)?;
                } else {
                    f(&rel, bytes)?;
                }
            }
        }
    }
    Ok(())
}

fn subdir_with_bin_flag(dir: &include_dir::Dir) -> bool {
    let _ = dir;
    true
}

/// List catalog items from the embedded official plugin packages.
pub fn list_catalog() -> Result<Vec<CatalogItem>, String> {
    let mut items = Vec::new();
    for d in embed::PLUGINS.dirs() {
        let Some(data) = d.get_file("manifest.yaml") else { continue };
        let mf: Manifest = match serde_yaml::from_str(std::str::from_utf8(data.contents()).unwrap_or("")) {
            Ok(m) => m,
            Err(_) => continue,
        };
        items.push(CatalogItem {
            name: mf.name,
            title: mf.title,
            version: mf.version,
            author: mf.author,
            description: mf.description,
            language: mf.language,
            installed: false,
        });
    }
    Ok(items)
}

pub fn catalog_contains(name: &str) -> bool {
    embed::PLUGINS.get_dir(name).is_some()
}

/// Copy an embedded plugin bundle into `PANEL_HOME/plugins/<name>`.
pub fn install_from_embed(home: &str, name: &str) -> Result<(), String> {
    let dir = match embed::PLUGINS.get_dir(name) {
        Some(d) => d,
        None => return Err(format!("插件包不存在: {}", name)),
    };
    let dest = PathBuf::from(home).join("plugins").join(name);
    let _ = std::fs::remove_dir_all(&dest);
    let mut f = |rel: &str, bytes: Vec<u8>| -> Result<(), String> {
        let target = dest.join(rel);
        if bytes.is_empty() && rel.ends_with('/') {
            return Ok(());
        }
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let is_bin = rel.contains("/bin/");
        std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
        if is_bin {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
        }
        Ok(())
    };
    let _ = subdir_with_bin_flag(dir);
    walk_embed(dir, true, &mut f)?;
    Ok(())
}

/// Parse a gzip+tar plugin package. Requires exactly one top-level directory
/// containing a `manifest.yaml`. Returns `(plugin_dir_name, rel->content)`.
pub fn unpack_plugin_package(data: &[u8]) -> Result<(String, std::collections::HashMap<String, Vec<u8>>), String> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut files: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    let mut top_dir: Option<String> = None;
    let mut total: u64 = 0;
    let mut bounded = tar::Archive::new(gz);
    for entry in bounded.entries().map_err(|e| format!("解压失败: {}", e))? {
        let mut entry = entry.map_err(|e| format!("解压失败: {}", e))?;
        let header = entry.header().path_bytes();
        let name = String::from_utf8_lossy(&header).to_string();
        // sanitize: clean and reject traversal
        let mut parts: Vec<&str> = name.split('/').collect();
        let cleaned: Vec<String> = parts.iter().map(|s| s.to_string()).collect();
        let mut temp = "".to_string();
        for p in &cleaned {
            match p.as_str() {
                "" | "." => {}
                ".." => return Err(format!("非法路径: {}", name)),
                _ => temp = if temp.is_empty() { p.clone() } else { format!("{}/{}", temp, p) },
            }
        }
        let clean = temp;
        if clean.starts_with('/') || clean.starts_with("..") {
            return Err(format!("非法路径: {}", name));
        }
        // split top-level
        let seg: Vec<&str> = clean.splitn(2, '/').collect();
        if seg.len() < 2 {
            continue; // top-level file ignored (only top dir matters)
        }
        match &top_dir {
            None => top_dir = Some(seg[0].to_string()),
            Some(t) if t != seg[0] => return Err("包内存在多个顶层目录".into()),
            _ => {}
        }
        let is_dir = entry.header().entry_type().is_dir();
        if is_dir {
            continue;
        }
        let mut content = Vec::new();
        entry.take(MAX_REMOTE_PLUGIN_SIZE).read_to_end(&mut content).map_err(|e| e.to_string())?;
        total += content.len() as u64;
        if total > MAX_REMOTE_PLUGIN_SIZE * 4 {
            return Err(format!("插件包解压后总大小超过上限 ({} MB)", (MAX_REMOTE_PLUGIN_SIZE * 4) >> 20));
        }
        files.insert(seg[1].to_string(), content);
    }
    let _ = &mut files;
    let top = top_dir.ok_or_else(|| "包内未找到插件目录".to_string())?;
    if !files.contains_key("manifest.yaml") {
        return Err("插件目录缺少 manifest.yaml".into());
    }
    Ok((top, files))
}

/// Install a URL-downloaded package into `PANEL_HOME/plugins/<name>`.
pub fn write_package(home: &str, name: &str, files: &std::collections::HashMap<String, Vec<u8>>) -> Result<(), String> {
    let dest = PathBuf::from(home).join("plugins").join(name);
    let _ = std::fs::remove_dir_all(&dest);
    for (rel, content) in files {
        let target = dest.join(rel);
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let is_bin = rel.starts_with("bin/");
        std::fs::write(&target, content).map_err(|e| e.to_string())?;
        if is_bin {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/// Helper re-export for handlers that need a manifest from the filesystem.
pub fn manifest_at(home: &str, name: &str) -> Result<Manifest, String> {
    load_manifest(&Path::new(home).join("plugins").join(name))
}