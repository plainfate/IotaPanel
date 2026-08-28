#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# 编译 legacy-demo（旧生态兼容性测试插件）
# 模拟旧 Go 生态里的预编译插件二进制：静态链接、零依赖，任意发行版可跑。
set -euo pipefail
cd "$(dirname "$0")"
gcc -static -O2 -o legacy-demo legacy.c
echo "compiled: $(pwd)/legacy-demo ($(du -h legacy-demo | cut -f1))"
