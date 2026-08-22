// SPDX-License-Identifier: GPL-3.0-or-later
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// MicroPanel is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// MicroPanel is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with MicroPanel.  If not, see <https://www.gnu.org/licenses/>.

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
