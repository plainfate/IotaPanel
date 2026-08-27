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

//! 工具函数：时间、环境变量、YAML 迷你解析、进程存活探测。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 当前 Unix 时间戳（秒）。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 当前 Unix 时间戳（毫秒）。
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// RFC3339 本地无关时间字符串（UTC，秒精度），与 Go `time.RFC3339` 兼容。
pub fn rfc3339(secs: i64) -> String {
    civil_from_secs(secs)
        .map(|(y, mo, d, h, mi, s)| format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

pub fn rfc3339_now() -> String {
    rfc3339(now_secs())
}

/// 秒时间戳转 (年,月,日,时,分,秒) —— UTC。算法：Howard Hinnant 的 civil_from_days。
#[allow(clippy::type_complexity)]
pub fn civil_from_secs(secs: i64) -> Option<(i64, u32, u32, u32, u32, u32)> {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    Some((y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32))
}

/// "YYYY-MM-DD HH:MM"（本地时区），目录列表展示用；失败回退 UTC 字符串。
pub fn human_mtime(mtime_secs: i64) -> String {
    // 与 Go 版一致取本地时区；容器里通常 TZ=UTC，直接用 UTC 表示
    match civil_from_secs(mtime_secs) {
        Some((y, mo, d, h, mi, _)) => format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, mi),
        None => "-".into(),
    }
}

/// 读环境变量或默认值。
pub fn env_or(key: &str, def: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| def.to_string())
}

/// 解析 KEY=VALUE 格式的 .env 文件为映射。
pub fn parse_env_file(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(data) = std::fs::read_to_string(path) {
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }
    map
}

/// 把一个键写入 .env 文件（保留其他行），文件不存在则创建。
pub fn set_env_var(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out: Vec<String> = Vec::new();
    let mut found = false;
    if let Ok(data) = std::fs::read_to_string(path) {
        for line in data.split('\n') {
            let trimmed = line.trim();
            if trimmed.starts_with(&format!("{}=", key)) {
                out.push(format!("{}={}", key, value));
                found = true;
            } else {
                out.push(line.to_string());
            }
        }
    }
    if !found {
        out.push(format!("{}={}", key, value));
    }
    // 去掉末尾多余空行后统一以单个换行结尾
    while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        out.pop();
    }
    std::fs::write(path, out.join("\n") + "\n")
}

// ---------- YAML 迷你子集 ----------
//
// 只支持 manifest.yaml / config.yaml 实际用到的形态：
//   key: value
//   key:                # 标量或嵌套
//     child: value
//   list:
//     - title: a
//       icon: b
//     - title: c
//   flag: true          # 布尔/数字/带引号字符串

/// 极简 YAML 解析结果节点。
#[derive(Debug, Clone)]
pub enum Yaml {
    Str(String),
    Bool(bool),
    Num(f64),
    List(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    pub fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Yaml::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Yaml::Bool(b) => Some(*b),
            Yaml::Str(s) => match s.as_str() {
                "true" | "yes" | "on" => Some(true),
                "false" | "no" | "off" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// 取字符串字段，不存在时给默认值（manifest/config 读取的主要入口）。
    pub fn str_or(&self, key: &str, def: &str) -> String {
        match self.get(key) {
            Some(Yaml::Str(s)) if !s.is_empty() => s.clone(),
            Some(Yaml::Num(n)) => n.to_string(),
            _ => def.to_string(),
        }
    }

    pub fn bool_or(&self, key: &str, def: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(def)
    }

    pub fn list(&self, key: &str) -> Vec<Yaml> {
        match self.get(key) {
            Some(Yaml::List(items)) => items.clone(),
            _ => Vec::new(),
        }
    }

    /// 取"对象列表"字段：manifest 的 menus、配置里的多段结构都用它。
    pub fn list_map(&self, key: &str) -> Vec<Vec<(String, Yaml)>> {
        self.get(key)
            .map(|v| v.as_map_list())
            .unwrap_or_default()
    }

    pub fn as_map_list(&self) -> Vec<Vec<(String, Yaml)>> {
        match self {
            Yaml::List(items) => items
                .iter()
                .filter_map(|it| match it {
                    Yaml::Map(m) => Some(m.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

struct Line {
    indent: usize,
    content: String,
}

/// 把迷你 YAML 文本解析为节点树。
pub fn parse_yaml(text: &str) -> Yaml {
    let lines: Vec<Line> = text
        .lines()
        .filter_map(|raw| {
            let trimmed_end = raw.trim_end();
            let no_comment = strip_comment(trimmed_end);
            let t = no_comment.trim();
            if t.is_empty() || t == "---" {
                return None;
            }
            let indent = no_comment.len() - no_comment.trim_start().len();
            Some(Line { indent, content: t.to_string() })
        })
        .collect();
    let mut pos = 0usize;
    parse_block(&lines, &mut pos, 0)
}

fn strip_comment(line: &str) -> String {
    // 简单策略：不在引号内的 # 起视为注释
    let mut out = String::new();
    let mut in_squote = false;
    let mut in_dquote = false;
    for ch in line.chars() {
        match ch {
            '\'' if !in_dquote => in_squote = !in_squote,
            '"' if !in_squote => in_dquote = !in_dquote,
            '#' if !in_squote && !in_dquote => break,
            _ => {}
        }
        out.push(ch);
    }
    out
}

fn parse_block(lines: &[Line], pos: &mut usize, indent: usize) -> Yaml {
    if *pos >= lines.len() {
        return Yaml::Map(Vec::new());
    }
    if lines[*pos].content.starts_with("- ") || lines[*pos].content == "-" {
        // 列表块
        let mut items = Vec::new();
        while *pos < lines.len() && lines[*pos].indent >= indent {
            let line = &lines[*pos];
            if !(line.content.starts_with("- ") || line.content == "-") {
                break;
            }
            let rest = line.content.strip_prefix("- ").unwrap_or("").to_string();
            let item_indent = line.indent + 2;
            if rest.contains(':') && !rest.ends_with(':') {
                // 内联首键：手工展开成一层缩进的伪行处理太复杂，
                // 这里直接把首键插入临时结构后继续消费同缩进行
                let mut inner: Vec<(String, Yaml)> = Vec::new();
                let (k, v) = split_kv(&rest);
                if v.is_empty() {
                    // 首键的值可能是嵌套块，暂不支持该形态（本项目未用到）
                    inner.push((k, Yaml::Str(String::new())));
                } else {
                    inner.push((k, scalar(&v)));
                }
                *pos += 1;
                while *pos < lines.len() && lines[*pos].indent >= item_indent {
                    let l = &lines[*pos];
                    if l.content.starts_with("- ") {
                        break;
                    }
                    if l.indent < item_indent {
                        break;
                    }
                    let (k2, v2) = split_kv(&l.content);
                    if v2.is_empty() {
                        let sub = parse_block(lines, pos, l.indent + 1);
                        inner.push((k2, sub));
                    } else {
                        inner.push((k2, scalar(&v2)));
                        *pos += 1;
                    }
                }
                items.push(Yaml::Map(inner));
                continue;
            } else if rest.is_empty() {
                // 值在下一层
                *pos += 1;
                items.push(parse_block(lines, pos, item_indent));
            } else {
                items.push(scalar(rest.trim()));
                *pos += 1;
            }
        }
        return Yaml::List(items);
    }

    // 映射块
    let mut map: Vec<(String, Yaml)> = Vec::new();
    while *pos < lines.len() {
        let line = &lines[*pos];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            // 异常深缩进：跳过防御
            *pos += 1;
            continue;
        }
        if line.content.starts_with("- ") {
            break; // 回到上层处理列表
        }
        let (k, v) = split_kv(&line.content);
        if v.is_empty() {
            // 嵌套块（更深缩进）或者空值
            let next_indent = lines.get(*pos + 1).map(|l| l.indent).unwrap_or(999);
            if next_indent > indent {
                *pos += 1;
                let child = parse_block(lines, pos, next_indent);
                map.push((k, child));
            } else {
                map.push((k, Yaml::Str(String::new())));
                *pos += 1;
            }
        } else {
            map.push((k, scalar(&v)));
            *pos += 1;
        }
    }
    Yaml::Map(map)
}

fn split_kv(line: &str) -> (String, String) {
    match line.split_once(':') {
        Some((k, v)) => (unquote(k.trim()), v.trim().to_string()),
        None => (unquote(line.trim()), String::new()),
    }
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').trim_matches('\'').to_string()
}

fn scalar(raw: &str) -> Yaml {
    let raw = unquote(raw.trim());
    match raw.as_str() {
        "true" | "True" | "TRUE" | "yes" | "on" => return Yaml::Bool(true),
        "false" | "False" | "FALSE" | "no" | "off" => return Yaml::Bool(false),
        "" => return Yaml::Str(String::new()),
        _ => {}
    }
    if let Ok(n) = raw.parse::<f64>() {
        if !raw.is_empty() && raw.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) {
            return Yaml::Num(n);
        }
    }
    Yaml::Str(raw)
}

// ---------- 进程工具 ----------

/// 进程是否存活（kill(pid, 0) 等价：读 /proc/<pid>/stat）。
#[cfg(unix)]
pub fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    Path::new(&format!("/proc/{}", pid)).exists()
}

/// 读进程启动时钟节拍（/proc/<pid>/stat 第 22 字段），防 PID 复用误杀。
#[cfg(unix)]
pub fn proc_start_tick(pid: i32) -> Option<u64> {
    let data = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let after_comm = data.rsplit_once(')').map(|x| x.1)?;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    fields.get(19)?.parse().ok()
}

/// 发信号优雅终止进程；最多等待 3 秒后 SIGKILL。
#[cfg(unix)]
pub fn terminate_process(pid: i32) {
    use std::io::Write;
    unsafe extern "C" {
        fn kill(pid: libc_pid_t, sig: i32) -> i32;
    }
    #[allow(non_camel_case_types)]
    type libc_pid_t = i32;

    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;

    if pid <= 0 {
        return;
    }
    let tick = proc_start_tick(pid);

    unsafe {
        if kill(pid as libc_pid_t, SIGTERM) != 0 {
            return; // 进程已不存在
        }
    }
    // 校验 PID 未被复用
    if let Some(t0) = tick {
        if proc_start_tick(pid) != Some(t0) {
            return;
        }
    }
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        match tick {
            Some(t0) => {
                if proc_start_tick(pid) != Some(t0) {
                    return; // 已退出
                }
            }
            None => {
                if !process_alive(pid) {
                    return;
                }
            }
        }
    }
    unsafe {
        kill(pid as libc_pid_t, SIGKILL);
    }
    let _ = std::io::stdout().flush();
}

#[cfg(not(unix))]
pub fn terminate_process(_pid: i32) {}

/// 目录大小（字节）。
pub fn dir_size(p: &PathBuf) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                total += dir_size(&path);
            } else if let Ok(md) = e.metadata() {
                total += md.len();
            }
        }
    }
    total
}
