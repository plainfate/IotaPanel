#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 plainfate <https://github.com/plainfate>
# ============================================================
# IotaPanel 一键安装脚本
#
# 用法（最常见）:
#   bash install.sh -d /data/panel
#     → 自动识别架构 → 下载核心二进制（约 9MB）→ 部署 → 注册 systemd → 启动
#     → 浏览器访问 http://<服务器IP>:8787 进入首次启动向导
#
# 其他用法:
#   bash install.sh -d /data/panel --port 8787          # 自定义端口
#   bash install.sh -d /data/panel --no-systemd         # 不注册 systemd（手动启动）
#   bash install.sh -d /data/panel --url https://...    # 指定二进制/发布包地址
#
# 特性：
#   - 安装位置完全自定义，不强制根分区
#   - 发布包自动做 SHA256 校验；二进制部署后自动自检（-version）
#   - 升级：再次运行本脚本，仅替换 bin/panel，.env / 数据库 / 插件均不受影响
#
# 说明：本脚本不做自动下载（无托管服务器）。
#       二进制来源只有两种：本目录 bin/panel（本地构建），或 --url 手动指定地址。
# ============================================================
set -euo pipefail

INSTALL_DIR="/data/panel"
PORT="8787"
USE_SYSTEMD=1
DOWNLOAD_URL=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<EOF
IotaPanel 一键安装脚本

用法:
  bash install.sh -d /data/panel [--port 8787] [--no-systemd] [--url URL]

选项:
  -d, --dir DIR     安装目录（默认 /data/panel，可任意分区）
  -p, --port PORT   面板监听端口（默认 8787）
  -n, --no-systemd  不注册 systemd 服务（手动启动）
      --url URL     手动指定二进制/发布包下载地址（可选）
  -h, --help        显示帮助
EOF
}

# 解析命令行参数
while [ $# -gt 0 ]; do
  case "$1" in
    -d|--dir)   INSTALL_DIR="$2"; shift 2 ;;
    -p|--port)  PORT="$2"; shift 2 ;;
    -n|--no-systemd) USE_SYSTEMD=0; shift ;;
    --url)      DOWNLOAD_URL="$2"; shift 2 ;;
    -h|--help)  usage; exit 0 ;;
    *) echo "未知参数: $1"; usage; exit 1 ;;
  esac
done

# 需要 root 权限（写 /etc/systemd 与绑定端口）
if [ "$(id -u)" != "0" ]; then
  echo "错误: 请以 root 运行（sudo bash install.sh ...）" >&2
  exit 1
fi

# ---------- 自动识别 CPU 架构 ----------
case "$(uname -m)" in
  x86_64|amd64)  ARCH="amd64" ;;
  aarch64|arm64) ARCH="arm64" ;;
  i386|i686)     ARCH="386" ;;
  *) echo "错误: 不支持的架构: $(uname -m)" >&2; exit 1 ;;
esac

echo "==> 安装目录: $INSTALL_DIR （架构: linux/$ARCH）"
mkdir -p "$INSTALL_DIR"/{bin,etc,data,logs/plugins,plugins}

# ---------- 部署核心二进制 ----------
BIN_SRC=""
# 二进制来源优先级：--url > 本地 bin/panel
if [ -n "$DOWNLOAD_URL" ]; then
  BIN_SRC="remote(${DOWNLOAD_URL%%/*})"
  case "$DOWNLOAD_URL" in
    *.tar.gz|*.tgz)
      # 发布包形式：iotapanel-<版本>-linux-<架构>.tar.gz
      echo "==> 下载发布包: $DOWNLOAD_URL"
      TMP="$(mktemp -d)"
      curl -fsSL -o "$TMP/pkg.tar.gz" "$DOWNLOAD_URL"
      # 若同目录存在 .sha256 则自动校验完整性
      if curl -fsSL -m 10 -o "$TMP/pkg.tar.gz.sha256" "$DOWNLOAD_URL.sha256" 2>/dev/null; then
        # 归一化校验文件：取哈希 + 重新指向本地文件名（源文件可能带目录前缀）
        HASH="$(awk '{print $1}' "$TMP/pkg.tar.gz.sha256")"
        if [ -n "$HASH" ]; then
          echo "$HASH  pkg.tar.gz" > "$TMP/pkg.tar.gz.sha256"
          (cd "$TMP" && sha256sum -c pkg.tar.gz.sha256) && echo "==> SHA256 校验通过" \
            || { echo "错误: SHA256 校验失败，请检查下载来源。" >&2; rm -rf "$TMP"; exit 1; }
        else
          echo "==> 警告: 校验文件格式无法识别，跳过校验"
        fi
      fi
      tar -xzf "$TMP/pkg.tar.gz" -C "$TMP"
      BIN_FILE="$(find "$TMP" -path "*/bin/panel" -type f | head -1)"
      [ -n "$BIN_FILE" ] || { echo "错误: 发布包中未找到 bin/panel。" >&2; rm -rf "$TMP"; exit 1; }
      cp "$BIN_FILE" "$INSTALL_DIR/bin/panel"
      rm -rf "$TMP"
      ;;
    *)
      # 单二进制形式：--url https://.../panel-linux-arm64
      echo "==> 下载核心二进制: $DOWNLOAD_URL"
      curl -fsSL -o "$INSTALL_DIR/bin/panel" "$DOWNLOAD_URL"
      ;;
  esac
elif [ -f "$SCRIPT_DIR/bin/panel" ]; then
  cp "$SCRIPT_DIR/bin/panel" "$INSTALL_DIR/bin/panel"
  BIN_SRC="local(./bin/panel)"
else
  echo "错误: 未找到本地二进制，也未提供 --url。" >&2
  echo "      请先本地构建: ./build.sh && bash install.sh -d $INSTALL_DIR" >&2
  echo "      或手动指定地址: bash install.sh -d $INSTALL_DIR --url https://.../panel-linux-$ARCH" >&2
  exit 1
fi
chmod +x "$INSTALL_DIR/bin/panel"

# ---------- 二进制自检（防架构不匹配/损坏） ----------
echo "==> 自检二进制: $("$INSTALL_DIR/bin/panel" -version 2>&1 || true)"
if ! "$INSTALL_DIR/bin/panel" -version >/dev/null 2>&1; then
  echo "错误: 二进制无法执行（可能架构不匹配或文件损坏），来源: $BIN_SRC" >&2
  exit 1
fi

# ---------- 生成 .env（已存在则保留，升级不覆盖） ----------
ENV_FILE="$INSTALL_DIR/etc/.env"
if [ ! -f "$ENV_FILE" ]; then
  JWT_SECRET="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  cat > "$ENV_FILE" <<EOF
# IotaPanel 环境配置（升级时不会覆盖本文件）
# LISTEN_ADDR 可用:
#   :$PORT        全部网卡, IPv4+IPv6 双栈（默认）
#   0.0.0.0:$PORT 仅 IPv4 全接口
#   127.0.0.1:$PORT 仅本机访问
PANEL_HOME=$INSTALL_DIR
LISTEN_ADDR=:$PORT
JWT_SECRET=$JWT_SECRET
IDLE_TIMEOUT=5m
EOF
  echo "==> 已生成 $ENV_FILE（含随机 JWT_SECRET）"
else
  echo "==> 已存在 $ENV_FILE，保留原配置"
fi

# ---------- 注册 systemd 服务 ----------
if [ "$USE_SYSTEMD" = "1" ] && command -v systemctl >/dev/null 2>&1; then
  UNIT=/etc/systemd/system/iotapanel.service
  cat > "$UNIT" <<EOF
# IotaPanel systemd 单元（由 install.sh 生成）
[Unit]
Description=IotaPanel - 极简微内核服务器面板
After=network.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/bin/panel
Restart=on-failure
RestartSec=3
KillSignal=SIGTERM
TimeoutStopSec=10
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable iotapanel >/dev/null 2>&1 || true
  systemctl restart iotapanel
  echo "==> systemd 服务已注册并启动: iotapanel"
else
  echo "==> 未注册 systemd，可手动启动: $INSTALL_DIR/bin/panel"
fi

# ---------- 创建 panel 命令（PATH 软链） ----------
ln -sf "$INSTALL_DIR/bin/panel" /usr/local/bin/panel 2>/dev/null || true
echo "==> 已创建命令: panel（任意目录可执行 panel start/stop/restart/status/log/uninstall）"

# ---------- 完成 ----------
IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
echo ""
echo "✅ IotaPanel 安装完成（二进制来源: $BIN_SRC）"
echo "   访问地址: http://${IP:-127.0.0.1}:$PORT"
echo "   首次访问将进入初始化向导（设置管理员账号 + 勾选基础插件）"
echo "   配置文件: $ENV_FILE"
