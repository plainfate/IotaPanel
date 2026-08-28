#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 一键部署脚本（自动识别系统 + 架构）
#   检测当前 Linux 发行版与 CPU 架构 → 从 GitHub Release 下载对应架构的
#   musl 静态包（任意发行版通用）→ 解压并运行包内 install.sh 完成安装。
#
# 用法:
#   curl -sSL https://raw.githubusercontent.com/plainfate/IotaPanel/rust-musl-rewrite/deploy.sh | bash
#   bash deploy.sh                        # 检测当前架构，装最新 release
#   bash deploy.sh --version v0.4.0       # 指定版本（不带 v 前缀亦可）
#   bash deploy.sh --repo owner/repo      # 其它仓库
#   bash deploy.sh --local ./release      # 用本地 release/ 目录安装（不联网）
#   bash deploy.sh --home /data/panel     # 数据目录
#   bash deploy.sh --prefix /usr          # 安装前缀
#
# 支持的目标架构（musl 静态，全 Linux 发行版通用）:
#   x86_64  (amd64)            → iotapanel-*-x86_64-musl.tar.gz
#   aarch64 (arm64)            → iotapanel-*-aarch64-musl.tar.gz
#   armv7   (armhf/armv7l)     → iotapanel-*-armv7-musl.tar.gz
#   i686    (i386)             → iotapanel-*-i686-musl.tar.gz

set -euo pipefail

REPO="plainfate/IotaPanel"
VERSION=""                 # 空 = latest
HOME_DIR=""
PREFIX=""
LOCAL_DIR=""

# ---------- 参数解析 ----------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)    REPO="$2"; shift 2;;
    --version) VERSION="${2#v}"; shift 2;;
    --home)    HOME_DIR="$2"; shift 2;;
    --prefix)  PREFIX="$2"; shift 2;;
    --local)   LOCAL_DIR="$2"; shift 2;;
    -h|--help) grep -E "^#   " "$0" | sed 's/^#   //'; exit 0;;
    *) echo "未知参数: $1" >&2; exit 1;;
  esac
done

err() { echo "!! $*" >&2; }
# 下载函数在命令替换中返回临时文件路径，因此日志必须走 stderr。
log() { echo "==> $*" >&2; }

# ---------- 系统检测 ----------
detect_os() {
  local os=""
  if [[ "$(uname -s)" != "Linux" ]]; then
    err "IotaPanel 目前仅支持 Linux。当前系统: $(uname -s)"
    return 1
  fi
  if [[ -r /etc/os-release ]]; then
    os="$(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-Linux}")"
  fi
  echo "${os:-Linux}"
}

# ---------- 架构检测（输出与发行包文件名一致的后缀）----------
detect_arch() {
  local m
  m="$(uname -m)"
  case "$m" in
    x64|x86_64|amd64) echo "x86_64";;
    aarch64|arm64)    echo "aarch64";;
    armv7l|armhf|armv7|armv6l) echo "armv7";;
    i686|i386|x86)    echo "i686";;
    riscv64)          echo "riscv64";;
    *) err "无法识别的架构: $m"; return 1;;
  esac
}

# ---------- 本地安装（从 tarball）----------
install_tarball() {
  local tarball="$1"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  log "解压 $tarball ..."
  tar -xzf "$tarball" -C "$tmp"
  local pkg
  pkg="$(find "$tmp" -maxdepth 1 -type d | tail -1)"
  local args=()
  [[ -n "$HOME_DIR" ]] && args+=(--home "$HOME_DIR")
  [[ -n "$PREFIX" ]] && args+=(--prefix "$PREFIX")
  log "运行 $(basename "$pkg")/install.sh ${args[*]} ..."
  ( cd "$pkg" && bash ./install.sh "${args[@]}" )
}

# ---------- 远程下载 ----------
download_from_github() {
  local arch="$1" ver="$2" url tarball asset_url
  if [[ -z "$ver" ]]; then
    log "获取 latest release ..."
    ver="$(curl -sSL -H "Accept: application/vnd.github+json" \
      "https://api.github.com/repos/$REPO/releases/latest" \
      2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
    [[ -z "$ver" ]] && { err "获取 latest release 失败"; return 1; }
  fi
  log "使用版本: $ver （架构后缀: $arch）"
  url="https://api.github.com/repos/$REPO/releases/tags/$ver"
  # 发行包命名: iotapanel-<ver>-<arch>-musl.tar.gz
  asset_url="$(curl -sSL -H "Accept: application/vnd.github+json" "$url" 2>/dev/null \
    | grep -oE "\"browser_download_url\" *: *\"[^\"]*-$arch-musl\\.tar\\.gz\"" \
    | sed -E 's/.*"browser_download_url" *: *"([^"]*)".*/\1/' | head -1)"
  if [[ -z "$asset_url" ]]; then
    err "release '$ver' 中没有 -$arch-musl.tar.gz 的产物；可能该架构尚未发布，或脚本识别错误"
    return 1
  fi
  # BusyBox mktemp 不支持 --suffix，模板后缀用 XXXXXX 兼容 Alpine。
  tarball="$(mktemp /tmp/iotapanel.XXXXXX)"
  log "下载 $asset_url"
  if ! curl -fsSL -o "$tarball" "$asset_url"; then
    rm -f "$tarball"
    return 1
  fi
  echo "$tarball"
}

# ---------- 主流程 ----------
OS="$(detect_os)" || exit 1
log "操作系统: $OS"
log "仓库: $REPO  数据目录: ${HOME_DIR:-<auto>}"

arch="$(detect_arch)" || exit 1
log "检测到架构: $(uname -m) → $arch"

if [[ -n "$LOCAL_DIR" ]]; then
  local_tb="$LOCAL_DIR/iotapanel-*-$arch-musl.tar.gz"
  tb="$(ls -1 $local_tb 2>/dev/null | head -1 || true)"
  if [[ -z "$tb" ]]; then
    err "本地目录未找到 $arch 包（期望 $local_tb）"
    exit 1
  fi
  install_tarball "$tb"
else
  tb="$(download_from_github "$arch" "$VERSION")" || exit 1
  install_tarball "$tb"
  rm -f "$tb"
fi

log "部署完成。请打开浏览器访问 http://<IP>:8787 完成初始化。"
