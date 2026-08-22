#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 plainfate <https://github.com/plainfate>
# ============================================================
# IotaPanel 发布打包脚本（多平台）
#
# 用法:
#   ./package.sh                              # 打包全部平台: linux-amd64 linux-arm64 windows-amd64 darwin-amd64
#   ./package.sh --targets linux-amd64,linux-arm64   # 只打指定平台
#   ./package.sh --version 0.3.0              # 自定义版本号
#
# 产物（dist/）:
#   iotapanel-<版本>-linux-<amd64|arm64>.tar.gz        # 含 install.sh（一键安装）
#   iotapanel-<版本>-windows-amd64.tar.gz             # 纯二进制 + 说明
#   iotapanel-<版本>-darwin-amd64.tar.gz              # 纯二进制 + 说明
#   各平台附带 .sha256 校验文件
#
# 说明: 各平台内嵌插件不同（依赖 unix 系统调用的插件不参与 Windows 构建）
# ============================================================
set -euo pipefail
cd "$(dirname "$0")"

VERSION=""
TARGETS=""

usage() {
  echo "用法: ./package.sh [--targets linux-amd64,linux-arm64,windows-amd64,darwin-amd64] [--version 0.3.0]"
}

# 解析命令行参数
while [ $# -gt 0 ]; do
  case "$1" in
    --targets) TARGETS="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "未知参数: $1"; usage; exit 1 ;;
  esac
done

# 版本号：优先 --version，否则从 config.go 读取
if [ -z "$VERSION" ]; then
  VERSION=$(grep -o 'Version = "[^"]*"' internal/config/config.go | head -1 | grep -o '"[^"]*"' | tr -d '"' || true)
fi
[ -z "$VERSION" ] && VERSION="0.3.0"

# 默认目标（含本机架构）
NATIVE_ARCH=$(uname -m)
case "$NATIVE_ARCH" in
  x86_64|amd64)  NATIVE_ARCH=amd64 ;;
  aarch64|arm64) NATIVE_ARCH=arm64 ;;
esac
if [ -z "$TARGETS" ]; then
  TARGETS="linux-amd64,linux-arm64,windows-amd64,darwin-amd64"
fi

export PATH=$PATH:/usr/local/go/bin
export GOPROXY="${GOPROXY:-https://goproxy.cn,direct}"
mkdir -p dist

# 每个平台的内嵌插件列表：
#   linux:   全部插件（含终端）
#   darwin:  不含终端（pty 仅 unix，但终端暂只内嵌 Linux 包）
#   windows: 仅纯标准库插件
plugin_list() {
  case "$1" in
    linux-*)   echo "file-manager resource-monitor hello terminal https-front" ;;
    darwin-*)  echo "file-manager resource-monitor hello https-front" ;;
    windows-*) echo "hello" ;;
    *)         echo "file-manager resource-monitor hello" ;;
  esac
}

pack_platform() {
  local os_arch="$1"
  local os="${os_arch%-*}"
  local arch="${os_arch#*-}"
  local plugins
  plugins="$(plugin_list "$os_arch")"

  echo ""
  echo "========== 构建 ${os}/${arch} (插件: ${plugins// /, }) =========="
  if ! GOOS="$os" GOARCH="$arch" PLUGINS="$plugins" ./build.sh > "/tmp/mp-build-$os_arch.log" 2>&1; then
    echo "编译失败，日志如下："; tail -20 "/tmp/mp-build-$os_arch.log"; exit 1
  fi

  local NAME="iotapanel-${VERSION}-${os}-${arch}"
  local PKG="dist/$NAME"
  rm -rf "$PKG"
  mkdir -p "$PKG/bin"
  local BINNAME="panel"
  [ "$os" = "windows" ] && BINNAME="panel.exe"
  cp "bin/panel" "$PKG/bin/$BINNAME"
  chmod +x "$PKG/bin/$BINNAME"
  cp README.md LICENSE "$PKG/"

  if [ "$os" = "linux" ]; then
    cp install.sh "$PKG/"
    chmod +x "$PKG/install.sh"
  else
    # 非 Linux 平台附上使用说明
    cat > "$PKG/README.$os.md" <<EOF
# IotaPanel ${VERSION} (${os}/${arch})

非 Linux 平台的简化版：核心功能完整（认证、网关、插件进程管理），
但仅内置兼容本平台的插件（${plugins// /、}）。

启动: $PKG/bin/$BINNAME
（默认监听 :8787，PANEL_HOME 可自定义；首次访问走 Web 初始化向导）
EOF
  fi

  tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
  # 校验文件用相对文件名（不带 dist/ 前缀），用户下载到任意目录即可 sha256sum -c
  (cd dist && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
  echo "    完成: dist/$NAME.tar.gz ($(du -h "dist/$NAME.tar.gz" | cut -f1))"
  echo "    校验: $(awk '{print $1}' "dist/$NAME.tar.gz.sha256")"
}

# 逐个平台打包
for t in ${TARGETS//,/ }; do
  pack_platform "$t"
done

# 恢复本机架构的 bin/panel（避免交叉编译产物留在本地无法运行）
echo ""
echo "==> 恢复本机架构 ($NATIVE_ARCH) 的 bin/panel …"
GOOS=linux GOARCH="$NATIVE_ARCH" PLUGINS="file-manager resource-monitor hello terminal https-front" ./build.sh > /tmp/mp-build-native.log 2>&1 || true

echo ""
echo "✅ 打包完成，产物位于 dist/"
