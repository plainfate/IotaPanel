#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 发布打包脚本（musl 静态，重写版，多架构）
#
# 用法:
#   ./build-release.sh                                     # 全部架构：x86_64/aarch64/armv7/i686
#   ./build-release.sh --arch x86_64-unknown-linux-musl    # 仅指定架构
#   ./build-release.sh --arch aarch64-unknown-linux-musl,armv7-unknown-linux-musleabihf
#
# 产物: release/iotapanel-<ver>-<arch>-musl.tar.gz
#   每个包内含 bin/（核心+5 个官方插件）、plugins/<name>/manifest.yaml、install.sh。
#
# 多架构策略：全部采用 *-unknown-linux-musl 静态目标，零动态依赖，
# 在 Alpine / Debian / Ubuntu / CentOS / Arch 等任意 Linux 发行版原生运行。
# 交叉编译依赖 musl.cc 命名的交叉工具链（aarch64-linux-musl-gcc 等），
# 脚本会自动把常见安装目录加入 PATH，也可用 MUSL_CROSS_ROOT 指定。

set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(pwd)"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)"
VERSION="${VERSION:-0.4.0}"

# 默认打包全部架构
DEFAULT_ARCHES="x86_64-unknown-linux-musl,aarch64-unknown-linux-musl,armv7-unknown-linux-musleabihf,i686-unknown-linux-musl"
ARCHES="$DEFAULT_ARCHES"
if [[ "${1:-}" == "--arch" ]]; then
  ARCHES="${2:-$DEFAULT_ARCHES}"
fi
IFS=',' read -ra ARCH_LIST <<< "$ARCHES"

# ---------- 交叉工具链 PATH ----------
if [[ -n "${MUSL_CROSS_ROOT:-}" ]]; then
  for d in "$MUSL_CROSS_ROOT"/*-cross/bin; do [[ -d "$d" ]] && PATH="$d:$PATH"; done
fi
for d in /opt/musl/*-cross/bin "$HOME"/musl-cross/*/bin /usr/local/musl/*/bin; do
  [[ -d "$d" ]] && PATH="$d:$PATH"
done
export PATH
echo ">> 工具链 PATH 片段: $(echo "$PATH" | tr ':' '\n' | grep -i musl | tr '\n' ' ')"

PLUGINS="hello resource-monitor terminal https-front mcp-agent"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

for TRIPLE in "${ARCH_LIST[@]}"; do
  TRIPLE="$(echo "$TRIPLE" | tr -d ' ')"
  echo "==================== 构建并打包 $TRIPLE ===================="
  ./build.sh --target "$TRIPLE" || { echo "!! $TRIPLE 构建失败，跳过"; continue; }

  ARCH="${TRIPLE%%-unknown-linux-*}"
  PKGDIR="$TMPDIR/iotapanel-$VERSION-$ARCH-musl"
  mkdir -p "$PKGDIR/bin" "$PKGDIR/plugins"

  cp -f bin/iotapanel "$PKGDIR/bin/"
  for p in $PLUGINS; do
    [[ -f "bin/iotapanel-plugin-$p" ]] && cp -f "bin/iotapanel-plugin-$p" "$PKGDIR/bin/"
  done
  ln -sf iotapanel "$PKGDIR/bin/panel"

  for p in $PLUGINS; do
    mkdir -p "$PKGDIR/plugins/$p"
    [[ -f "plugins/$p/manifest.yaml" ]] && cp -f "plugins/$p/manifest.yaml" "$PKGDIR/plugins/$p/manifest.yaml"
  done

  cp -f install.sh "$PKGDIR/install.sh"
  chmod +x "$PKGDIR/install.sh"

  cat > "$PKGDIR/README-RELEASE.md" <<EOF
# IotaPanel $VERSION ($TRIPLE)

Rust 版即用静态包：核心 + 全部 5 个官方插件，musl 静态链接，零动态依赖
（Alpine / Debian / Ubuntu / CentOS / Arch 均可原生运行）。

## 一键部署
    ./install.sh

## 手动运行（前台）
    PANEL_HOME=\$HOME/.iotapanel bin/iotapanel serve

访问 http://<IP>:8787 进入初始化向导。
架构: $TRIPLE
EOF

  OUTDIR="$ROOT/release"
  mkdir -p "$OUTDIR"
  OUT="$OUTDIR/iotapanel-$VERSION-$ARCH-musl.tar.gz"
  tar -C "$TMPDIR" -czf "$OUT" "$(basename "$PKGDIR")"
  echo "  已生成: $OUT ($(du -h "$OUT" | cut -f1))"
done

echo ">> release/ 内容:"
ls -la "$ROOT/release/" 2>/dev/null | grep -E "tar.gz|total" || true
