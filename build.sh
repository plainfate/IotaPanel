#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 构建脚本
#   1) cargo build --release（工作区：sdk + core + 全部官方插件）
#   2) 组装 bin/ 目录（面板二进制 + 各插件二进制）
#
# 前端静态资源在编译期经 include_str!/include_bytes! 直接内嵌到各自二进制，
# 无需单独生成嵌入表；改动 web/ 或 plugins/*/web 后重跑本脚本即可。
#
# 用法:
#   ./build.sh                # release 构建并组装 bin/
#   ./build.sh --debug        # debug 构建
#   ./build.sh --package      # 额外生成 tar.gz 发布包

set -euo pipefail
cd "$(dirname "$0")"

ROOT="$(pwd)"
MODE="${1:-release}"

echo ">> [1/3] cargo build ($MODE)"
export PATH="$PATH:$HOME/.cargo/bin"

if [[ "$MODE" == "--debug" ]]; then
  cargo build
else
  cargo build --release
fi

echo ">> [2/3] 组装 bin/"
mkdir -p bin
RELDIR=target/release
if [[ "$MODE" == "--debug" ]]; then RELDIR=target/debug; fi
cp -f "$RELDIR/iotapanel" bin/iotapanel
for name in hello file-manager resource-monitor terminal https-front mcp-agent; do
  [ -f "$RELDIR/iotapanel-plugin-$name" ] && cp -f "$RELDIR/iotapanel-plugin-$name" "bin/iotapanel-plugin-$name"
done
ln -sf iotapanel bin/panel 2>/dev/null || true

echo ">> [3/3] bin/ 内容:"
ls -la bin/

if [[ "${2:-}" == "--package" ]]; then
  echo ">> 打包 iotapanel-$(bin/iotapanel version | awk '{print $2}').tar.gz"
  tar -czf "iotapanel-${VERSION:-release}.tar.gz" bin install.sh README.md 2>/dev/null || \
  tar -czf iotapanel-release.tar.gz bin install.sh README.md 2>/dev/null || true
  ls -la *.tar.gz 2>/dev/null
fi

echo ">> 完成."
