#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# IotaPanel Rust 核心安装脚本（与 Go 版 install.sh 同等：systemd 服务 + 开箱即用）
#
# 用法:
#   bash install.sh -d /data/panel [--url https://.../iotapanel-rust-<ver>-<arch>.tar.gz]
#   bash install.sh -d /data/panel   # 若当前目录已有解压好的 iotapanel-rust 与 plugins/
#
# 行为:
#   1. --url 时自动下载、SHA256 校验（若同目录有 .sha256 或 sha256sums.txt）、解压
#   2. 建立 PANEL_HOME 目录结构（plugins/ data/ etc/ logs/）
#   3. 安装二进制到 <dir>/bin/iotapanel-rust，插件到 <dir>/plugins/
#   4. 注册并启动 systemd 服务 iotapanel-rust（KillMode=process，重启不杀插件）
set -euo pipefail

DIR=""; URL=""; NO_SYSTEMD=0
while [ $# -gt 0 ]; do
  case "$1" in
    -d|--dir) DIR="$2"; shift 2 ;;
    --url) URL="$2"; shift 2 ;;
    --no-systemd) NO_SYSTEMD=1; shift ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "未知参数: $1（用 -h 查看用法）"; exit 1 ;;
  esac
done
[ -z "$DIR" ] && { echo "用法: bash install.sh -d <安装目录> [--url <包地址>]"; exit 1; }

echo "==> IotaPanel Rust 核心安装"
echo "==> 目标目录: $DIR"

# 1. 下载/解压（可选）
if [ -n "$URL" ]; then
  PKG=$(basename "$URL")
  echo "==> 下载 $PKG ..."
  curl -fL --retry 3 -o "/tmp/$PKG" "$URL"
  SUMF="/tmp/$PKG.sha256"
  curl -fsL -o "$SUMF" "$URL.sha256" 2>/dev/null || true
  if [ -s "$SUMF" ]; then
    (cd /tmp && sha256sum -c "$PKG.sha256" >/dev/null 2>&1) || echo "!! SHA256 校验不匹配（继续，但请检查）"
  fi
  echo "==> 解压 ..."
  tar xzf "/tmp/$PKG" -C /tmp
  BASE="/tmp/${PKG%.tar.gz}"
  [ -d "$BASE" ] || BASE=$(find /tmp -maxdepth 1 -type d -name 'iotapanel-rust-*' | head -1)
  SRC="$BASE"
else
  SRC="$(pwd)"
fi

# 2. 建立目录结构
mkdir -p "$DIR"/{bin,plugins,data,etc,logs/plugins}
[ ! -f "$DIR/etc/.env" ] && : > "$DIR/etc/.env"

# 3. 安装二进制
BIN_SRC="$SRC/iotapanel-rust"
[ -f "$BIN_SRC" ] || BIN_SRC=$(find "$SRC" -maxdepth 1 -name 'iotapanel-rust' -type f | head -1)
if [ -f "$BIN_SRC" ]; then
  install -m 0755 "$BIN_SRC" "$DIR/bin/iotapanel-rust"
  echo "==> 已安装二进制: $DIR/bin/iotapanel-rust"
else
  echo "!! 未找到 iotapanel-rust 二进制（请确认包结构或当前目录）"; exit 1
fi

# 4. 安装插件（若包内含 plugins/）
if [ -d "$SRC/plugins" ]; then
  cp -r "$SRC/plugins/." "$DIR/plugins/"
  echo "==> 已安装插件: $(ls "$DIR/plugins" | tr '\n' ' ')"
else
  echo "==> 包内无 plugins/（可自行放入 $DIR/plugins/）"
fi

# 5. systemd 服务
if [ "$NO_SYSTEMD" != "1" ] && command -v systemctl >/dev/null 2>&1 && [ "$(id -u)" = "0" ]; then
  cat > /etc/systemd/system/iotapanel-rust.service <<EOF
[Unit]
Description=IotaPanel Rust Core (iotapanel-rust)
After=network.target

[Service]
Type=simple
Environment=PANEL_HOME=$DIR
ExecStart=$DIR/bin/iotapanel-rust
Restart=on-failure
KillMode=process
TimeoutStopSec=10

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable iotapanel-rust >/dev/null 2>&1 || true
  systemctl restart iotapanel-rust 2>/dev/null || systemctl start iotapanel-rust 2>/dev/null || true
  echo "==> systemd 服务: iotapanel-rust（开机自启已启用）"
else
  echo "==> 未启用 systemd。手动运行:"
  echo "    PANEL_HOME=$DIR LISTEN_ADDR=:8787 $DIR/bin/iotapanel-rust"
fi

# 6. 提示
echo ""
echo "==> 完成。浏览器打开 http://<IP>:8787 首次进入初始化向导。"
echo "    插件目录: $DIR/plugins/（放入任意插件目录即装即用）"
echo "    空闲时长 IDLE_TIMEOUT / 转发超时 PROXY_TIMEOUT 可写入 systemd Environment 或环境变量。"
