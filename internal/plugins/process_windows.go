// SPDX-License-Identifier: GPL-3.0-or-later
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// IotaPanel is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// IotaPanel is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with IotaPanel.  If not, see <https://www.gnu.org/licenses/>.

//go:build windows

package plugins

// processAlive 判断进程是否存活。
// Windows 无 kill(pid,0)，这里始终返回 true：等满优雅退出窗口后由 SIGKILL 兜底。
func processAlive(pid int) bool { return true }

// procStartTick Windows 上无法轻量读取进程启动时间，返回 unknown。
// killProc 将退化为仅按存活状态处理（Windows 上 3 秒窗口内 PID 复用的概率很低）。
func procStartTick(pid int) (uint64, bool) { return 0, false }
