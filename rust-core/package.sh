#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# IotaPanel Rust 核心打包脚本：核心二进制 + 官方插件一起打包（开箱即用）
# 与 Go 版 package.sh 同等地位；macOS 因 Apple SDK 闭源不在本脚本产出。
# 用法: ./package.sh   （产物 dist/iotapanel-rust-<版本>-<平台>.tar.gz，附 .sha256）
set -euo pipefail
cd "$(dirname "$0")"
source "$HOME/.cargo/env" 2>/dev/null || true
GOROOT_BIN="/tmp/go-toolchain/go/bin"
[ -x "$GOROOT_BIN/go" ] || GOROOT_BIN="$(dirname "$(command -v go)")"
export PATH="$GOROOT_BIN:$PATH" GOPROXY="${GOPROXY:-https://goproxy.cn,direct}" GODEBUG=netdns=go+4

VERSION="0.4.1"
LINUX_PLUGINS="file-manager resource-monitor hello terminal https-front mcp-agent"
WIN_PLUGINS="hello"
rm -rf dist && mkdir -p dist

# 构建官方 Go 插件到 $1/<name>/（bin + manifest），按 GOOS/GOARCH
build_plugins() {
  local GOOS=$1 GOARCH=$2 OUT=$3
  local plist; if [ "$GOOS" = "windows" ]; then plist=$WIN_PLUGINS; else plist=$LINUX_PLUGINS; fi
  for p in $plist; do
    mkdir -p "$OUT/$p/bin"
    local ext=""; [ "$GOOS" = "windows" ] && ext=".exe"
    (cd "../plugins/$p" && GOOS="$GOOS" GOARCH="$GOARCH" go build -trimpath -o "$OUT/$p/bin/$p$ext" .)
    cp "../plugins/$p/manifest.yaml" "$OUT/$p/manifest.yaml"
    echo "  插件: $p ($GOOS/$GOARCH)"
  done
}

# 打包一个平台：$1=核心二进制 $2=GOOS $3=GOARCH $4=包目录名
pack() {
  local bin=$1 GOOS=$2 GOARCH=$3 pkgdir=$4
  mkdir -p "$pkgdir"
  cp "$bin" "$pkgdir/iotapanel-rust"
  build_plugins "$GOOS" "$GOARCH" "$pkgdir/plugins"
  (cd dist && tar czf "$pkgdir.tar.gz" "$(basename "$pkgdir")")
  (cd dist && sha256sum "$(basename "$pkgdir").tar.gz" | awk '{print $1"  "$2}' > "$(basename "$pkgdir").tar.gz.sha256")
  echo "  完成: $pkgdir.tar.gz"
}

# 1. 原生（本机架构）
NATIVE=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
echo "== 原生 $NATIVE =="
cargo build --release
pack "target/release/iotapanel-rust" "linux" "$NATIVE" "iotapanel-rust-${VERSION}-linux-${NATIVE}"

# 2. linux/amd64 交叉
if command -v x86_64-linux-gnu-gcc >/dev/null 2>&1; then
  rustup target add x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
  echo "== 交叉 linux/amd64 =="
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc cargo build --release --target x86_64-unknown-linux-gnu
  pack "target/x86_64-unknown-linux-gnu/release/iotapanel-rust" "linux" "amd64" "iotapanel-rust-${VERSION}-linux-amd64"
fi

# 3. windows/amd64 交叉
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true
  echo "== 交叉 windows/amd64 =="
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo build --release --target x86_64-pc-windows-gnu
  cp target/x86_64-pc-windows-gnu/release/iotapanel-rust.exe "dist/iotapanel-rust-${VERSION}-windows-amd64/iotapanel-rust.exe" 2>/dev/null || true
  mkdir -p "dist/iotapanel-rust-${VERSION}-windows-amd64"
  cp target/x86_64-pc-windows-gnu/release/iotapanel-rust.exe "dist/iotapanel-rust-${VERSION}-windows-amd64/iotapanel-rust.exe"
  build_plugins windows amd64 "dist/iotapanel-rust-${VERSION}-windows-amd64/plugins"
  (cd dist && tar czf "iotapanel-rust-${VERSION}-windows-amd64.tar.gz" "iotapanel-rust-${VERSION}-windows-amd64")
  (cd dist && sha256sum "iotapanel-rust-${VERSION}-windows-amd64.tar.gz" | awk '{print $1"  "$2}' > "iotapanel-rust-${VERSION}-windows-amd64.tar.gz.sha256")
  echo "  完成: iotapanel-rust-${VERSION}-windows-amd64.tar.gz"
fi

echo ""
echo "== 产物 =="
ls -lh dist/*.tar.gz | awk '{print $5, $9}'
echo "说明：macOS 需在 Mac 或 GitHub Actions macOS runner 构建（Apple SDK 闭源）。"
