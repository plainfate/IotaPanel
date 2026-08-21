#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 plainfate <https://github.com/plainfate>
# ============================================================
# MicroPanel 构建脚本
#
# 步骤：
#   1. 编译示例插件（Go 插件 → 单个二进制；任意语言插件均可）
#      产物写入 internal/embed/plugins/，编译期内嵌进核心二进制
#   2. 编译面板核心 → bin/panel（单一自包含二进制）
#
# 环境变量:
#   PLUGINS  参与内嵌的插件列表（空格分隔）。
#            默认含全部插件；Windows 目标需排除依赖 unix 系统调用的插件，
#            由 package.sh 按平台传入，例如: PLUGINS="hello"
# ============================================================
set -euo pipefail
cd "$(dirname "$0")"

GO="${GO:-go}"
OUT=bin

# 默认插件（Linux/macOS 全量；Windows 目标由 package.sh 显式传入）
PLUGINS="${PLUGINS:-file-manager resource-monitor hello terminal}"

echo "[1/4] 清理旧的内嵌插件包…"
rm -rf internal/embed/plugins
mkdir -p internal/embed/plugins

# 编译 Go 插件
# 插件支持任意语言：把对应产物放进 plugins/<name>/bin/ 并在 manifest 声明入口即可。
# 内嵌前 gzip 压缩（-k 保留源文件），二进制体积更小；安装时核心自动解压。
for p in $PLUGINS; do
  [ -d "plugins/$p" ] || { echo "跳过不存在的插件: $p"; continue; }
  echo "[2/4] 编译并压缩插件: $p"
  mkdir -p "internal/embed/plugins/$p/bin"
  (cd "plugins/$p" && "$GO" build -trimpath -ldflags="-s -w" -o "../../internal/embed/plugins/$p/bin/$p" .)
  gzip -9 -k -f "internal/embed/plugins/$p/bin/$p"
  rm -f "internal/embed/plugins/$p/bin/$p"   # 只保留 .gz，核心安装时解压
  cp "plugins/$p/manifest.yaml" "internal/embed/plugins/$p/manifest.yaml"
done

# 编译面板核心
echo "[3/4] 编译面板核心…"
mkdir -p "$OUT"
"$GO" build -trimpath -ldflags="-s -w" -o "$OUT/panel" ./cmd/panel

echo "[4/4] 完成: $OUT/panel ($(du -h "$OUT/panel" | cut -f1))"
echo "      运行开发版: PANEL_HOME=/tmp/mp-dev LISTEN_ADDR=127.0.0.1:8787 $OUT/panel"
