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

//! 内嵌资源访问器。
//!
//! 实际数据表在 `embedded_data.rs`（由 build.sh 在编译前生成；
//! 仓库内置一张空表占位，保证纯后端开发构建可用）。

pub fn file(path: &str) -> Option<&'static [u8]> {
    crate::embedded_data::FILES
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.data)
}

/// 枚举某目录前缀下的直接子项名（如 "plugins" → hello, terminal…）。
pub fn list_dir(prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in crate::embedded_data::FILES {
        let Some(rest) = f.path.strip_prefix(prefix) else { continue };
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            continue;
        }
        let top = rest.split('/').next().unwrap();
        if seen.insert(top.to_string()) {
            out.push(top.to_string());
        }
    }
    out.sort();
    out
}

pub fn all_paths() -> impl Iterator<Item = &'static str> {
    crate::embedded_data::FILES.iter().map(|f| f.path)
}
