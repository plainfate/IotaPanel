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

//go:build windows

package plugins

// processAlive 判断进程是否存活。
// Windows 无 kill(pid,0)，这里始终返回 true：等满优雅退出窗口后由 SIGKILL 兜底。
func processAlive(pid int) bool { return true }

// procStartTick Windows 上无法轻量读取进程启动时间，返回 unknown。
// killProc 将退化为仅按存活状态处理（Windows 上 3 秒窗口内 PID 复用的概率很低）。
func procStartTick(pid int) (uint64, bool) { return 0, false }
