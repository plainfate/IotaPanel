#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# IotaPanel 端到端冒烟测试
#
# 覆盖：
#   1. 核心启动 / 初始化向导
#   2. 创建管理员 + 从内嵌表安装官方插件(hello)
#   3. 登录 / 状态 / 插件列表
#   4. 网关反向代理 /p/hello/（含插件自动冷启动）
#   5. 旧生态兼容性：远程安装旧格式插件包(tar.gz + manifest.yaml + 预编译二进制)，
#      启动并验证网关反代 —— 证明旧 Go 生态插件在新 Rust 核心上可用
#   6. 登出
#
# 用法: bash tests/smoke.sh   （需先 ./build.sh --musl 生成 bin/）
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd)"

BIN="$ROOT/bin/iotapanel"
[[ -x "$BIN" ]] || { echo "!! 未找到 bin/iotapanel，请先 ./build.sh --musl"; exit 1; }

PORT=18787
BASE="http://127.0.0.1:$PORT"
HOME_DIR="$(mktemp -d)"
CJ="$HOME_DIR/cj"
PANEL_PID=""
HTTP_PID=""
FAIL=0

cleanup() {
  [[ -n "$HTTP_PID" ]] && kill "$HTTP_PID" 2>/dev/null || true
  if [[ -n "$PANEL_PID" ]]; then
    # 优雅停止并等待面板退出（插件优雅退出最长 3s），确保端口释放，避免级联 bind 冲突
    kill "$PANEL_PID" 2>/dev/null || true
    for _ in $(seq 1 60); do
      kill -0 "$PANEL_PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -9 "$PANEL_PID" 2>/dev/null || true
  fi
  rm -rf "$HOME_DIR"
}
trap cleanup EXIT

pass() { echo "  ✅ $1"; }
fail() { echo "  ❌ $1"; FAIL=1; }
check() { # check <描述> <命令...>
  local desc="$1"; shift
  if "$@" >/dev/null 2>&1; then pass "$desc"; else fail "$desc"; fi
}

echo "===== IotaPanel 端到端冒烟测试 ====="

echo ">> [1] 启动核心 (PANEL_HOME=$HOME_DIR, :$PORT)"
LISTEN_ADDR="127.0.0.1:$PORT" PANEL_HOME="$HOME_DIR" "$BIN" serve >"$HOME_DIR/panel.out" 2>&1 &
PANEL_PID=$!

wait_for() { for _ in $(seq 1 100); do curl -sf "$1" >/dev/null 2>&1 && return 0; sleep 0.2; done; return 1; }
wait_for "$BASE/api/setup/state" || {
  echo "!! 核心未就绪"
  echo "--- ps ---"; ps aux | grep -E "iotapanel|panel" | grep -v grep || echo "(无 iotapanel 进程)"
  echo "--- ss ---"; ss -tlnp 2>/dev/null | grep -E "$PORT" || echo "(端口未监听)"
  echo "--- panel.out ---"; cat "$HOME_DIR/panel.out"
  echo "--- logs ---"; cat "$HOME_DIR/logs/panel.log" 2>/dev/null || true
  exit 1
}
pass "核心已启动并响应"

echo ">> [2] 初始化向导"
state="$(curl -sf "$BASE/api/setup/state")"
echo "    setup state: $state"
check "未初始化 (configured=false)" grep -q '"configured":false' <<<"$state"

echo ">> [3] 创建管理员 + 安装 bundled 插件(hello)"
curl -sf -X POST "$BASE/api/setup/start" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"test123456","plugins":["hello"]}' >/dev/null
for _ in $(seq 1 60); do
  st="$(curl -sf "$BASE/api/setup/status")"
  echo "$st" | grep -q '"running":true' && sleep 0.3 && continue
  echo "$st" | grep -q '"complete":true' && break
  sleep 0.3
done
echo "    setup status: $st"
check "初始化完成" grep -q '"complete":true' <<<"$st"
check "无错误" bash -c "grep -q '\"error\":\"\"' <<< '$st'"

echo ">> [4] 登录"
curl -sf -c "$CJ" -X POST "$BASE/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"test123456"}' | grep -q '"ok":true'
pass "登录成功（已种 cookie）"

echo ">> [5] /api/status"
st="$(curl -sf -b "$CJ" "$BASE/api/status")"
echo "    $st"
check "版本为 0.4.0" grep -q '"version":"0.4.0"' <<<"$st"
check "hello 已登记" grep -q '"plugins_installed":[1-9]' <<<"$st"

echo ">> [6] 网关冷启动 /p/hello/api/info"
info="$(curl -sf -b "$CJ" "$BASE/p/hello/api/info")"
echo "    $info"
check "hello 环境注入正确" bash -c "grep -q '\"plugin.name\":\"hello\"' <<< '$info' && grep -q '\"panel.home\":\"$HOME_DIR\"' <<< '$info'"

echo ">> [7] /api/plugins 状态"
pl="$(curl -sf -b "$CJ" "$BASE/api/plugins")"
echo "    $pl"
check "hello 运行中" grep -q '"running":true' <<<"$pl"
check "hello 菜单注入" grep -q '"title":"Hello"' <<<"$pl"

echo ">> [8] 网关 UI 页"
check "/p/hello/ 返回页面" bash -c "curl -sf -b '$CJ' '$BASE/p/hello/' | grep -q 'Hello'"

echo ">> [9] 旧生态兼容：编译 legacy 预编译插件（静态 C 二进制）"
if command -v gcc >/dev/null 2>&1; then
  bash tests/legacy-plugin/build.sh
  PKGDIR="$HOME_DIR/pkg"
  mkdir -p "$PKGDIR/legacy-demo/bin"
  cp tests/legacy-plugin/manifest.yaml "$PKGDIR/legacy-demo/"
  cp tests/legacy-plugin/legacy-demo "$PKGDIR/legacy-demo/bin/"
  tar -C "$PKGDIR" -czf "$HOME_DIR/legacy-demo.tar.gz" legacy-demo
  echo "    旧格式包: $HOME_DIR/legacy-demo.tar.gz"
  pass "legacy 插件包已生成（tar.gz + manifest.yaml + 预编译二进制）"
else
  fail "缺少 gcc，跳过 legacy 插件测试"
fi

echo ">> [10] 旧生态兼容：经 /api/store/install-url 远程安装旧插件包"
python3 -m http.server --directory "$HOME_DIR" 18888 >/dev/null 2>&1 &
HTTP_PID=$!
sleep 0.5
resp="$(curl -sf -b "$CJ" -X POST "$BASE/api/store/install-url" \
  -H 'Content-Type: application/json' \
  -d '{"url":"http://127.0.0.1:18888/legacy-demo.tar.gz"}')"
echo "    $resp"
check "安装成功" bash -c "grep -q '\"ok\":true' <<< '$resp' && grep -q 'legacy-demo' <<< '$resp'"
kill "$HTTP_PID" 2>/dev/null || true; HTTP_PID=""

echo ">> [11] 启动 legacy 插件并验证网关"
resp="$(curl -sf -b "$CJ" -X POST "$BASE/api/plugins/legacy-demo/start" \
  -H 'Content-Type: application/json' -d '{}')"
echo "    start: $resp"
check "插件进程已拉起" bash -c "grep -q '\"port\":19[0-9][0-9][0-9]' <<< '$resp'"
sleep 0.3
lgi="$(curl -sf -b "$CJ" "$BASE/p/legacy-demo/api/info")"
echo "    gateway: $lgi"
check "旧插件经网关可访问" bash -c "grep -q '\"name\":\"legacy-demo\"' <<< '$lgi'"
check "旧插件拿到 Go 版一致的注入变量" bash -c "grep -q '\"plugin_name\":\"legacy-demo\"' <<< '$lgi' && grep -q '\"panel_home\":\"$HOME_DIR\"' <<< '$lgi'"
check "legacy UI 页" bash -c "curl -sf -b '$CJ' '$BASE/p/legacy-demo/' | grep -q 'Legacy Plugin OK'"

echo ">> [12] 插件列表确认"
pl="$(curl -sf -b "$CJ" "$BASE/api/plugins")"
echo "    $pl"
check "legacy-demo 在列表且运行中" bash -c "grep -q '\"name\":\"legacy-demo\"' <<< '$pl' && grep -q '\"running\":true' <<< '$pl'"

echo ">> [13] 登出"
check "logout" bash -c "curl -sf -b '$CJ' -X POST '$BASE/api/logout' | grep -q '\"ok\":true'"

echo ">> [14] 核心日志尾部"
tail -5 "$HOME_DIR/logs/panel.log" 2>/dev/null | sed 's/^/    /' || true

echo
if [[ "$FAIL" == "0" ]]; then
  echo "===== ✅ 全部测试通过 ====="
else
  echo "===== ❌ 存在失败用例 ====="
  exit 1
fi
