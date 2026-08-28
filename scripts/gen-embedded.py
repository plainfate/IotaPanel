#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 内嵌资源表生成器
#
# 生成 core/src/embedded_data.rs，把以下内容在编译期内嵌进核心二进制：
#   - web/              面板前端（index/login/setup.html、css/js/svg）
#   - plugins/*/manifest.yaml   插件元信息（商城目录用）
#   - plugins/*/web/**          插件前端页面
#   - plugins/*/bin/*.gz        插件二进制（gzip 压缩，安装时自动解压）
#
# 由 build.sh 在编译核心前调用；仓库里保留一份生成的表，改 web/ 或插件后重跑即可。
# 与旧版差异：旧表只内嵌 manifest（插件装了没二进制可用），本版补齐 web 与 bin。

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "core", "src", "embedded_data.rs")

SKIP_PLUGIN_PREFIXES = ("src/",)
SKIP_PLUGIN_FILES = {"Cargo.toml"}


def walk_files(base: str):
    """返回 base 下所有文件的相对路径（/ 分隔，排序稳定）。"""
    out = []
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames.sort()
        for fn in sorted(filenames):
            rel = os.path.relpath(os.path.join(dirpath, fn), base)
            out.append(rel.replace(os.sep, "/"))
    return out


def build():
    if not os.path.isdir(os.path.join(ROOT, "web")):
        sys.exit("未找到 web/ 目录，请在仓库根目录运行")

    entries = []

    # ---- 核心前端 ----
    for rel in walk_files(os.path.join(ROOT, "web")):
        entries.append(f'    asset!("{rel}", "{rel}"),')

    # ---- 官方插件（跳过 .deprecated/ 等存档目录） ----
    plugins_dir = os.path.join(ROOT, "plugins")
    for name in sorted(os.listdir(plugins_dir)):
        if name.startswith("."):  # 跳过 .deprecated 等隐藏/存档目录
            continue
        pdir = os.path.join(plugins_dir, name)
        if not os.path.isdir(pdir):
            continue
        for rel in walk_files(pdir):
            if rel in SKIP_PLUGIN_FILES or rel.startswith(SKIP_PLUGIN_PREFIXES):
                continue
            entries.append(
                f'    asset!("plugins/{name}/{rel}", "../plugins/{name}/{rel}"),'
            )

    header = """// 核心内嵌资源表（由 scripts/gen-embedded.py 自动生成，勿手改；改资源后重跑 build.sh）。
#![allow(dead_code)]
pub struct EmbeddedFile {
    pub path: &'static str,
    pub data: &'static [u8],
}

macro_rules! asset {
    ($rel:expr, $file:expr) => {
        EmbeddedFile { path: $rel, data: include_bytes!(concat!("../../web/", $file)) }
    };
}

pub static FILES: &[EmbeddedFile] = &[
"""
    footer = "];\n"

    with open(OUT, "w", encoding="utf-8") as f:
        f.write(header)
        f.write("\n".join(entries))
        f.write("\n" + footer)

    print(f"generated {os.path.relpath(OUT, ROOT)}: {len(entries)} files")
    for e in entries:
        print("  " + e.strip())


if __name__ == "__main__":
    build()
