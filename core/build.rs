// SPDX-License-Identifier: Apache-2.0
//
// 核心构建脚本：编译前自动生成内嵌资源表 core/src/embedded_data.rs。
//
// 这样保证：
//   - 全新克隆直接 `cargo build` 也能编译（web/ 与 plugins/ 变化会自动触发重生成）；
//   - 插件二进制（plugins/<name>/bin/<name>.gz）由 build.sh 先 gzip 好，
//     build.rs 会把它一并纳入嵌入表，核心即能自举安装官方插件。
//   - 若环境无 python3，则沿用仓库内已提交的精简表（仅 web + manifest）。

use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest_dir).parent().unwrap().to_path_buf();

    // web/ 与 plugins/ 任一文件变化都要重生成
    println!("cargo:rerun-if-changed=../web");
    println!("cargo:rerun-if-changed=../plugins");

    let gen = root.join("scripts/gen-embedded.py");
    if gen.exists() {
        let ok = Command::new("python3")
            .arg(&gen)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            // 生成文件本身变化也要触发核心重编译
            println!("cargo:rerun-if-changed=src/embedded_data.rs");
        } else {
            eprintln!("[build.rs] 警告: python3 生成内嵌资源表失败，沿用已提交的精简表");
        }
    }
}
