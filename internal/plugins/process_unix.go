// SPDX-License-Identifier: Apache-2.0
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//go:build !windows

package plugins

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"syscall"
)

// processAlive 判断进程是否存活（unix：kill(pid, 0)）。
func processAlive(pid int) bool {
	return syscall.Kill(pid, 0) == nil
}

// procStartTick 读取 /proc/<pid>/stat 中的进程启动时钟节拍（整体第 22 字段）。
// 用于在发信号前确认 PID 仍是当初那个进程，防止 PID 被系统回收复用后误杀无关进程。
func procStartTick(pid int) (uint64, bool) {
	data, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return 0, false
	}
	// 格式：pid (comm) state ppid ... starttime ...；comm 以 ')' 结束，
	// 括号后第 1 个字段是 state（整体第 3 字段），starttime 是整体第 22 字段 → 下标 19。
	s := string(data)
	i := strings.LastIndexByte(s, ')')
	if i < 0 || i+2 >= len(s) {
		return 0, false
	}
	fields := strings.Fields(s[i+2:])
	if len(fields) < 20 {
		return 0, false
	}
	tick, err := strconv.ParseUint(fields[19], 10, 64)
	if err != nil {
		return 0, false
	}
	return tick, true
}
