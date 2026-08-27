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

//! 插件 manifest.yaml 解析（与 Go 版字段完全一致）。

use iotapanel_sdk::util::{parse_yaml, Yaml};
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Menu {
    pub title: String,
    pub icon: String,
    pub path: String,
    pub section: String,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub name: String,
    pub title: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub language: String,
    pub bind: String,
    pub command: String,
    pub args: Vec<String>,
    pub keepalive: bool,
    /// "" = 需面板登录；"none" = 免面板登录（插件自鉴权，如 MCP /mcp）
    pub auth: String,
    pub menus: Vec<Menu>,
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        let data = std::fs::read_to_string(dir.join("manifest.yaml"))
            .map_err(|e| format!("读取 manifest.yaml 失败: {}", e))?;
        let y = parse_yaml(&data);
        let name = y.str_or("name", "");
        let command = y.str_or("command", "");
        if name.is_empty() {
            return Err("manifest.yaml 缺少 name".into());
        }
        if command.is_empty() {
            return Err("manifest.yaml 缺少 command".into());
        }
        let mut mf = Manifest {
            name: name.clone(),
            title: y.str_or("title", &name),
            version: y.str_or("version", ""),
            author: y.str_or("author", ""),
            description: y.str_or("description", ""),
            language: y.str_or("language", ""),
            bind: y.str_or("bind", "127.0.0.1"),
            command: command.trim_start_matches("./").to_string(),
            args: Vec::new(),
            keepalive: y.bool_or("keepalive", false),
            auth: y.str_or("auth", ""),
            menus: Vec::new(),
        };
        // command 可能带行内注释残留（迷你 YAML 已剥离 # 注释），再兜底清理空白
        mf.command = mf.command.trim().to_string();
        for item in y.list_map("menus") {
            let m = Yaml::Map(item);
            mf.menus.push(Menu {
                title: m.str_or("title", ""),
                icon: m.str_or("icon", ""),
                path: m.str_or("path", "/"),
                section: m.str_or("section", "tools"),
            });
        }
        Ok(mf)
    }

    /// 序列化回 JSON（/api/plugins 用）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "title": self.title,
            "version": self.version,
            "author": self.author,
            "description": self.description,
            "language": self.language,
            "auth": self.auth,
            "keepalive_manifest": self.keepalive,
            "menus": self.menus.iter().map(|m| serde_json::json!({
                "title": m.title, "icon": m.icon, "path": m.path, "section": m.section,
            })).collect::<Vec<_>>(),
        })
    }
}
