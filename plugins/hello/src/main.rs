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

//! Hello 演示插件（纯 Rust 重写）。
//! 极简保活示例：展示插件以独立同级进程运行，并回显面板注入的环境变量。
//! 内嵌 web 静态页，经面板网关 /p/hello/ 访问。

use iotapanel_sdk::http::{Request, Response};

const INDEX_HTML: &str = include_str!("../web/index.html");

fn handle(req: &Request) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET" | "HEAD", "/api/info") => Response::json(&info_json()),
        ("GET" | "HEAD", _) => Response::html(INDEX_HTML), // 根与任何子路径都回首页
        _ => Response::json_err(404, "not found"),
    }
}

fn info_json() -> serde_json::Value {
    let vars = [
        ("PLUGIN_PORT", "plugin.port"),
        ("PLUGIN_BIND", "plugin.bind"),
        ("PLUGIN_NAME", "plugin.name"),
        ("PLUGIN_HOME", "plugin.home"),
        ("PANEL_HOME", "panel.home"),
    ];
    let env_map: serde_json::Map<String, serde_json::Value> = vars
        .iter()
        .filter_map(|(k, key)| {
            std::env::var(k)
                .ok()
                .map(|v| (key.to_string(), serde_json::Value::String(v)))
        })
        .collect();
    serde_json::json!({
        "name": "hello",
        "version": env!("CARGO_PKG_VERSION"),
        "language": "rust",
        "env": serde_json::Value::Object(env_map),
    })
}

fn main() {
    let bind = std::env::var("PLUGIN_BIND").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = std::env::var("PLUGIN_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(19003);

    let handler = |req: &Request| handle(req);
    eprintln!("[hello] listening on {}:{}", bind, port);
    if let Err(e) = iotapanel_sdk::http::serve(&bind, port, handler) {
        eprintln!("[hello] server error: {}", e);
        std::process::exit(1);
    }
}