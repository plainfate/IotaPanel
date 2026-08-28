#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 发布打包脚本（musl 静态，重写版）
#
# 用法:
#   ./build-release.sh                                     # x86_64 musl
#   ./build-release.sh --arch x86_64-unknown-linux-musl    # 指定架构
#   ./build-release.sh --arch aarch64-unknown-linux-musl,armv7-unknown-linux-musleabihf
#
# 产物: release/iotapanel-<ver>-<arch>-musl.tar.gz
# 每个包内含 bin/（核心+6 插件）、plugins/<name>/manifest.yaml、install.sh。
#
# 与旧版差异：
#   - 旧版默认 x86_64-unknown-linux-gnu（glibc），在 Alpine(musl) 上插件无法启动；
#     本版全部改为 *-unknown-linux-musl 静态目标。
#   - 交叉编译其它架构前需先安装对应 musl 工具链（见 DEVELOPMENT.md）。

set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(pwd)"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' core/Cargo.toml | head -1)"
VERSION="${VERSION:-0.4.0}"

ARCHES="x86_64-unknown-linux-musl"
if [[ "${1:-}" == "--arch" ]]; then
  ARCHES="${2:-$ARCHES}"
fi
IFS=',' read -ra ARCH_LIST <<< "$ARCHES"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

for TRIPLE in "${ARCH_LIST[@]}"; do
  TRIPLE="$(echo "$TRIPLE" | tr -d ' ')"
  echo "==================== 构建并打包 $TRIPLE ===================="
  case "$TRIPLE" in
    aarch64-unknown-linux-musl)
      export CC_aarch64_unknown_linux_musl="${CC_aarch64_unknown_linux_musl:-aarch64-linux-musl-gcc}"
      ;;
    armv7-unknown-linux-musleabihf)
      export CC_armv7_unknown_linux_musleabihf="${CC_armv7_unknown_linux_musleabihf:-arm-linux-musleabihf-gcc}"
      ;;
  esac
  ./build.sh --target "$TRIPLE" || { echo "!! $TRIPLE 构建失败，跳过"; continue; }

  ARCH="${TRIPLE%-linux-*}"
  PKGDIR="$TMPDIR/iotapanel-$VERSION-$ARCH-musl"
  mkdir -p "$PKGDIR/bin" "$PKGDIR/plugins"

  cp -f bin/iotapanel "$PKGDIR/bin/"
  for p in hello file-manager resource-monitor terminal https-front mcp-agent; do
    [[ -f "bin/iotapanel-plugin-$p" ]] && cp -f "bin/iotapanel-plugin-$p" "$PKGDIR/bin/"
  done
  ln -sf iotapanel "$PKGDIR/bin/panel"

  for p in hello file-manager resource-monitor terminal https-front mcp-agent; do
    mkdir -p "$PKGDIR/plugins/$p"
    cp -f "plugins/$p/manifest.yaml" "$PKGDIR/plugins/$p/manifest.yaml"
  done

  cp -f install.sh "$PKGDIR/install.sh"
  chmod +x "$PKGDIR/install.sh"

  cat > "$PKGDIR/README-RELEASE.md" <<EOF
# IotaPanel $VERSION ($TRIPLE)

Rust 版即用静态包：核心 + 全部 6 个官方插件，musl 静态链接，零动态依赖
（Alpine / Debian / Ubuntu / CentOS 均可原生运行）。

## 一键部署
    sudo ./install.sh

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
