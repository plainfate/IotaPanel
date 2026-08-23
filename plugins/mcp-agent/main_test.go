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

package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// TestCallToolGating 验证工具权限开关：enable_read / enable_write / allow_shell。
func TestCallToolGating(t *testing.T) {
	home := t.TempDir()
	cfg := Config{EnableRead: false, EnableWrite: false, AllowShell: false}

	if _, err := callTool("get_status", map[string]any{}, home, cfg, nil); err == nil || !strings.Contains(err.Error(), "只读工具已关闭") {
		t.Fatalf("read tool should be gated: %v", err)
	}
	if _, err := callTool("plugin_action", map[string]any{"plugin": "x", "action": "start"}, home, cfg, nil); err == nil || !strings.Contains(err.Error(), "写操作已关闭") {
		t.Fatalf("write tool should be gated: %v", err)
	}
	if _, err := callTool("run_command", map[string]any{"command": "echo hi"}, home, cfg, nil); err == nil || !strings.Contains(err.Error(), "run_command 未启用") {
		t.Fatalf("shell tool should be gated: %v", err)
	}

	cfg = Config{EnableRead: true, EnableWrite: true, AllowShell: true}
	if _, err := callTool("get_status", map[string]any{}, home, cfg, nil); err != nil {
		t.Fatalf("read tool should work: %v", err)
	}
	out, err := callTool("run_command", map[string]any{"command": "echo hi"}, home, cfg, nil)
	if err != nil || !strings.Contains(out, "hi") {
		t.Fatalf("run_command should work: %q %v", out, err)
	}
}

// TestMCPDispatch 验证 MCP 协议分发：令牌校验 / tools/list / 未知方法。
func TestMCPDispatch(t *testing.T) {
	token := "testtoken1234567890"
	home := t.TempDir()
	cfg := Config{EnableRead: true, EnableWrite: true}
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { mcpHandler(w, r, token, home, cfg, nil) })

	// 错误令牌 → 401
	req := httptest.NewRequest("POST", "/mcp", strings.NewReader(`{"jsonrpc":"2.0","id":1,"method":"ping"}`))
	req.Header.Set("Authorization", "Bearer wrong")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != http.StatusUnauthorized {
		t.Fatalf("bad token should 401, got %d", rec.Code)
	}

	// tools/list → 包含全部工具
	req = httptest.NewRequest("POST", "/mcp", strings.NewReader(`{"jsonrpc":"2.0","id":2,"method":"tools/list"}`))
	req.Header.Set("Authorization", "Bearer "+token)
	rec = httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	for _, want := range []string{"get_status", "list_plugins", "get_logs", "get_metrics", "plugin_action", "run_command"} {
		if !strings.Contains(rec.Body.String(), want) {
			t.Fatalf("tools/list missing %q: %s", want, rec.Body.String())
		}
	}

	// 未知方法 → JSON-RPC 错误码
	req = httptest.NewRequest("POST", "/mcp", strings.NewReader(`{"jsonrpc":"2.0","id":3,"method":"nope"}`))
	req.Header.Set("Authorization", "Bearer "+token)
	rec = httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if !strings.Contains(rec.Body.String(), "-32601") {
		t.Fatalf("unknown method should return -32601: %s", rec.Body.String())
	}
}
