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

package api

// 系统操作：重启面板、查看核心日志。

import (
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// handleSystemRestart 触发面板重启（异步）：
//   - systemd 安装：systemctl restart iotapanel
//   - 非 systemd：调用自身 CLI restart（停止旧进程 + 分离式重拉）
func (s *Server) handleSystemRestart(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "msg": "重启已触发，约 2 秒后恢复，请稍后刷新页面"})
	go func() {
		time.Sleep(500 * time.Millisecond) // 确保响应已发出
		exe, err := os.Executable()
		if err != nil {
			return
		}
		exec.Command(exe, "restart").Start()
	}()
}

// handleLog 返回核心日志 logs/panel.log 的最后 150 行。
func (s *Server) handleLog(w http.ResponseWriter, r *http.Request) {
	path := filepath.Join(s.cfg.Home, "logs", "panel.log")
	data, err := os.ReadFile(path)
	if err != nil {
		writeJSON(w, http.StatusOK, map[string]string{"log": ""})
		return
	}
	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > 150 {
		lines = lines[len(lines)-150:]
	}
	writeJSON(w, http.StatusOK, map[string]string{"log": strings.Join(lines, "\n")})
}
