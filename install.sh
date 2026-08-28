#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 一键安装脚本
#   把 bin/ 下的面板与插件二进制安装到目标目录，生成 systemd 服务并启动。
#
# 用法:
#   ./install.sh                    # 安装到默认目录并启动
#   ./install.sh --home /data/panel # 指定数据目录
#   ./install.sh --prefix /usr      # 二进制安装前缀
#   ./install.sh --nosystemd        # 不写 systemd 服务
#   ./install.sh --uninstall        # 卸载（保留数据）

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
SERVICE=iotapanel

PANEL_HOME="${PANEL_HOME:-}"
PREFIX="${PREFIX:-/usr/local}"
NOSYSTEMD=0
DO_UNINSTALL=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --home)    PANEL_HOME="$2"; shift 2;;
    --prefix)  PREFIX="$2"; shift 2;;
    --nosystemd) NOSYSTEMD=1; shift;;
    --uninstall) DO_UNINSTALL=1; shift;;
    -h|--help) sed -n '2,20p' "$0"; exit 0;;
    *) echo "未知参数: $1"; exit 1;;
  esac
done

if [[ "$DO_UNINSTALL" == "1" ]]; then
  echo "==> 卸载 IotaPanel"
  if command -v systemctl >/dev/null 2>&1 && [[ -f /etc/systemd/system/$SERVICE.service ]]; then
    systemctl disable --now "$SERVICE" 2>/dev/null || true
    rm -f "/etc/systemd/system/$SERVICE.service"
    systemctl daemon-reload 2>/dev/null || true
  fi
  rm -f "$PREFIX/bin/panel" "$PREFIX/bin/$SERVICE"
  echo "已卸载。数据目录 "$PANEL_HOME" 保留；如需彻底删除请手动执行 rm -rf "$PANEL_HOME""
  exit 0
fi

if [[ -z "$PANEL_HOME" ]]; then
  if [[ -w /data ]]; then PANEL_HOME=/data/panel
  else PANEL_HOME="$HOME/.iotapanel"; fi
fi

BIN_DIR="$PREFIX/bin"
echo "==> IotaPanel 安装"
echo "   数据目录: $PANEL_HOME"
echo "   二进制:   $BIN_DIR"

if [[ ! -f "$ROOT/bin/$SERVICE" ]]; then
  echo "!! 未找到 bin/$SERVICE，请先运行 ./build.sh"; exit 1
fi

mkdir -p "$BIN_DIR"
cp -f "$ROOT/bin/$SERVICE" "$BIN_DIR/$SERVICE"
chmod +x "$BIN_DIR/$SERVICE"
ln -sf "$SERVICE" "$BIN_DIR/panel"

# 插件二进制约装到数据目录（面板冷启动/保活自动拉起）
PLUGIN_DIR="$PANEL_HOME/plugins"
for p in hello file-manager resource-monitor terminal https-front mcp-agent; do
  mkdir -p "$PLUGIN_DIR/$p/bin"
  if [[ -f "$ROOT/bin/iotapanel-plugin-$p" ]]; then
    cp -f "$ROOT/bin/iotapanel-plugin-$p" "$PLUGIN_DIR/$p/bin/$p"
    chmod +x "$PLUGIN_DIR/$p/bin/$p"
  fi
done

# 初始化配置
etc="$PANEL_HOME/etc"
mkdir -p "$etc" "$PANEL_HOME/logs" "$PANEL_HOME/data"
if [[ ! -f "$etc/.env" ]]; then
  printf 'PANEL_HOME=%s\nLISTEN_ADDR=:8787\nIDLE_TIMEOUT=1800\nPORT_POOL_START=19000\nPORT_POOL_END=19999\n' "$PANEL_HOME" > "$etc/.env"
fi
echo "$PANEL_HOME" > /tmp/iotapanel-home 2>/dev/null || true

if [[ "$NOSYSTEMD" == "0" ]] && command -v systemctl >/dev/null 2>&1; then
  cat > "/etc/systemd/system/$SERVICE.service" <<SVC
[Unit]
Description=IotaPanel Control Panel
After=network.target

[Service]
Type=simple
ExecStart=$BIN_DIR/$SERVICE serve
Environment=PANEL_HOME=$PANEL_HOME
Restart=on-failure
RestartSec=3
LimitNOFILE=10240

[Install]
WantedBy=multi-user.target
SVC
  systemctl daemon-reload 2>/dev/null || true
  systemctl enable "$SERVICE" 2>/dev/null || true
  systemctl restart "$SERVICE" 2>/dev/null || true
  echo "==> 已通过 systemd 启动。访问 http://<IP>:8787"
  echo "   日志: journalctl -u $SERVICE -f"
elif [[ "$NOSYSTEMD" == "0" ]] && command -v rc-service >/dev/null 2>&1; then
  # Alpine / OpenRC：supervise-daemon 托管（进程退出自动拉起）
  cat > "/etc/init.d/$SERVICE" <<SVC
#!/sbin/openrc-run

name="IotaPanel Control Panel"
description="IotaPanel web control panel"
command="$BIN_DIR/$SERVICE"
command_args="serve"
command_background="yes"
pidfile="/run/$SERVICE.pid"
output_log="/var/log/$SERVICE.log"
error_log="/var/log/$SERVICE.log"

depend() {
    need net
}

start_pre() {
    export PANEL_HOME=$PANEL_HOME
    checkpath --directory --owner root:root /run /var/log
}
SVC
  chmod +x "/etc/init.d/$SERVICE"
  rc-update add "$SERVICE" default 2>/dev/null || true
  rc-service "$SERVICE" restart 2>/dev/null || true
  echo "==> 已通过 OpenRC 启动。访问 http://<IP>:8787"
  echo "   日志: tail -f /var/log/$SERVICE.log"
else
  echo "==> 无 systemd/OpenRC 或已禁用，请手动启动:"
  echo "   PANEL_HOME=$PANEL_HOME $BIN_DIR/$SERVICE serve"
fi

echo "==> 完成。"
