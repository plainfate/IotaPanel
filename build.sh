#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 构建脚本（重写版）
#
# 用法:
#   ./build.sh                          # native release（本机快速构建/开发）
#   ./build.sh --musl                   # x86_64-unknown-linux-musl 静态交叉编译（Alpine 部署推荐）
#   ./build.sh --target <triple>        # 指定任意 target（如 aarch64-unknown-linux-musl）
#   ./build.sh --debug                  # debug 构建（可与 --musl 组合）
#   ./build.sh --package                # 额外生成 tar.gz 发布包（含 install.sh）
#
# 构建流程（四步）:
#   1) 编译 SDK + 全部官方插件（不编 core，保证插件二进制先就位）
#   2) 插件二进制 gzip 成 plugins/<name>/bin/<name>.gz，生成 core/src/embedded_data.rs
#      （面板初始化向导/商店安装插件时，核心从内嵌表解压出插件可执行文件）
#   3) 编译核心（此时已把前端 + 插件包全部内嵌）
#   4) 组装 bin/：iotapanel + iotapanel-plugin-* + panel(软链)
#
# 为什么默认推荐 --musl：
#   旧版按 glibc（x86_64-unknown-linux-gnu）编译，跑在 Alpine(musl) 上会因缺
#   `__res_init` 等符号而无法启动插件；musl 静态链接后零动态依赖，任意发行版可用。

set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(pwd)"
export PATH="$PATH:$HOME/.cargo/bin"

MODE="release"
TARGET=""
PKG=0

# 统一参数解析（支持 --target <triple> 两段式）
while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)   MODE="debug"; shift ;;
    --musl)    TARGET="x86_64-unknown-linux-musl"; shift ;;
    --target)  TARGET="${2:-}"; shift 2 ;;
    --package) PKG=1; shift ;;
    *)         echo "未知参数: $1"; exit 1 ;;
  esac
done

if [[ -n "$TARGET" ]]; then
  echo ">> 目标平台: $TARGET"
  # 若无显式指定交叉编译器，则按约定在 PATH 中查找 musl.cc 命名的工具链。
  case "$TARGET" in
    x86_64-unknown-linux-musl)
      export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-musl-gcc}"
      export AR_x86_64_unknown_linux_musl="${AR_x86_64_unknown_linux_musl:-musl-ar}"
      ;;
    aarch64-unknown-linux-musl)
      export CC_aarch64_unknown_linux_musl="${CC_aarch64_unknown_linux_musl:-aarch64-linux-musl-gcc}"
      export AR_aarch64_unknown_linux_musl="${AR_aarch64_unknown_linux_musl:-aarch64-linux-musl-ar}"
      ;;
    armv7-unknown-linux-musleabihf)
      export CC_armv7_unknown_linux_musleabihf="${CC_armv7_unknown_linux_musleabihf:-arm-linux-musleabihf-gcc}"
      export AR_armv7_unknown_linux_musleabihf="${AR_armv7_unknown_linux_musleabihf:-arm-linux-musleabihf-ar}"
      ;;
    i686-unknown-linux-musl)
      export CC_i686_unknown_linux_musl="${CC_i686_unknown_linux_musl:-i486-linux-musl-gcc}"
      export AR_i686_unknown_linux_musl="${AR_i686_unknown_linux_musl:-i486-linux-musl-ar}"
      ;;
  esac
fi

CARGO_ARGS=(build)
[[ "$MODE" == "release" ]] && CARGO_ARGS+=(--release)
[[ -n "$TARGET" ]] && CARGO_ARGS+=(--target "$TARGET")

RELDIR="target/${TARGET:+$TARGET/}${MODE}"

echo ">> [1/4] 编译 SDK + 插件: cargo ${CARGO_ARGS[*]} --workspace --exclude iotapanel-core"
cargo "${CARGO_ARGS[@]}" --workspace --exclude iotapanel-core

echo ">> [2/4] 生成内嵌资源表 core/src/embedded_data.rs"
for name in hello resource-monitor terminal https-front mcp-agent; do
  src="$RELDIR/iotapanel-plugin-$name"
  if [[ -f "$src" ]]; then
    mkdir -p "plugins/$name/bin"
    gzip -c -n -f "$src" > "plugins/$name/bin/$name.gz"   # -n 去时间戳，构建可复现
    echo "   gzip: plugins/$name/bin/$name.gz ($(du -h "$src" | cut -f1))"
  fi
done
python3 scripts/gen-embedded.py

echo ">> [3/4] 编译核心: cargo ${CARGO_ARGS[*]} -p iotapanel-core"
cargo "${CARGO_ARGS[@]}" -p iotapanel-core

echo ">> [4/4] 组装 bin/"
mkdir -p bin
cp -f "$RELDIR/iotapanel" bin/iotapanel
for name in hello resource-monitor terminal https-front mcp-agent; do
  [[ -f "$RELDIR/iotapanel-plugin-$name" ]] && cp -f "$RELDIR/iotapanel-plugin-$name" "bin/iotapanel-plugin-$name"
done
ln -sf iotapanel bin/panel
ls -la bin/

if [[ "$PKG" == "1" ]]; then
  VER="$(bin/iotapanel version | awk '{print $2}')"
  VER="${VER:-0.4.0}"
  if [[ -n "$TARGET" ]]; then
    SUFFIX="${TARGET%-linux-*}"            # x86_64 / aarch64 / armv7 ...
    OUT="iotapanel-${VER}-${SUFFIX}-musl.tar.gz"
  else
    OUT="iotapanel-${VER}-native.tar.gz"
  fi
  tar -czf "$OUT" bin install.sh README.md 2>/dev/null || true
  echo ">> 发布包: $OUT"
fi

echo ">> 完成。"
