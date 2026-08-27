#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# 双核心方案的 Rust 核心打包脚本：与 Go 版 package.sh 各自独立出包。
# 用法: ./package.sh   （产物在 dist/，附 sha256）
set -euo pipefail
cd "$(dirname "$0")"
source "$HOME/.cargo/env" 2>/dev/null || true

VERSION="0.4.0"
rm -rf dist && mkdir -p dist

# 1. 原生（本机架构）
echo "== 构建原生 $($(command -v rustc) --version | awk '{print $2}') =="
cargo build --release
NATIVE=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
cp target/release/iotapanel-rust "dist/iotapanel-rust-${VERSION}-linux-${NATIVE}"

# 2. linux/amd64 交叉（需 gcc-x86-64-linux-gnu + libc6-dev-amd64-cross）
if command -v x86_64-linux-gnu-gcc >/dev/null 2>&1; then
  rustup target add x86_64-unknown-linux-gnu >/dev/null 2>&1 || true
  echo "== 交叉编译 linux/amd64 =="
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc cargo build --release --target x86_64-unknown-linux-gnu
  cp target/x86_64-unknown-linux-gnu/release/iotapanel-rust "dist/iotapanel-rust-${VERSION}-linux-amd64"
fi

# 3. windows/amd64 交叉（需 gcc-mingw-w64-x86-64）
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
  rustup target add x86_64-pc-windows-gnu >/dev/null 2>&1 || true
  echo "== 交叉编译 windows/amd64 =="
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc cargo build --release --target x86_64-pc-windows-gnu
  mkdir -p "dist/iotapanel-rust-${VERSION}-windows-amd64"
  cp target/x86_64-pc-windows-gnu/release/iotapanel-rust.exe "dist/iotapanel-rust-${VERSION}-windows-amd64/iotapanel-rust.exe"
fi

# 4. 打包 + 校验
cd dist
for f in iotapanel-rust-*; do
  [ -f "$f" ] && tar czf "$f.tar.gz" "$f"
  [ -d "$f" ] && tar czf "$f.tar.gz" "$f"
done
sha256sum *.tar.gz > sha256sums.txt
sha256sum -c sha256sums.txt
echo ""
echo "== 产物 =="
ls -lh *.tar.gz | awk '{print $5, $9}'
echo "说明：macOS 需在 Mac 或 GitHub Actions macOS runner 上构建（Apple SDK 闭源）。"