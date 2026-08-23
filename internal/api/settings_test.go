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

import (
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"iotapanel/internal/config"
	"iotapanel/internal/db"
	"iotapanel/internal/plugins"
)

// newTestServer 构造一个最小可用的 Server（真实 DB + 管理器），用于 handler 集成测试。
func newTestServer(t *testing.T) *Server {
	t.Helper()
	home := t.TempDir()
	cfg := &config.Config{
		Home:        home,
		JWTSecret:   "test-secret",
		IdleTimeout: 5 * time.Minute,
		PortLo:      19000,
		PortHi:      19999,
	}
	database, err := db.Open(home)
	if err != nil {
		t.Fatalf("db.Open: %v", err)
	}
	mgr := plugins.NewManager(home, cfg.IdleTimeout, cfg.PortLo, cfg.PortHi, database, slog.New(slog.DiscardHandler))
	return NewServer(cfg, database, mgr, slog.New(slog.DiscardHandler))
}

// TestSettingsPutRejectsPartialCommit 回归测试：请求中只要有一个字段非法，整个请求应失败且
// 不得把前面合法字段写入（之前会有部分提交：idle 已写入后才在校验 theme 时返回 400）。
func TestSettingsPutRejectsPartialCommit(t *testing.T) {
	s := newTestServer(t)
	_ = s.db.SetSetting("idle_timeout_minutes", "30")

	body := `{"idle_timeout_minutes":99,"theme":"not-a-theme"}`
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPut, "/api/settings", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	s.handleSettingsPut(rec, req)

	if rec.Code != http.StatusBadRequest {
		t.Fatalf("status = %d, want 400 (invalid theme should fail the whole request)", rec.Code)
	}
	// idle 不得被写入 99
	if v, _ := s.db.GetSetting("idle_timeout_minutes"); v != "30" {
		t.Fatalf("idle_timeout_minutes = %q after failed request, want \"30\" (partial commit)", v)
	}
	// theme 也不得被写入
	if v, ok := s.db.GetSetting("theme"); ok {
		t.Fatalf("theme unexpectedly written: %q", v)
	}
}

// TestSettingsPutAppliesValidFields 合法请求应正常生效。
func TestSettingsPutAppliesValidFields(t *testing.T) {
	s := newTestServer(t)
	body := `{"idle_timeout_minutes":45,"theme":"ocean"}`
	rec := httptest.NewRecorder()
	req := httptest.NewRequest(http.MethodPut, "/api/settings", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	s.handleSettingsPut(rec, req)

	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200", rec.Code)
	}
	if v, _ := s.db.GetSetting("idle_timeout_minutes"); v != "45" {
		t.Fatalf("idle_timeout_minutes = %q, want 45", v)
	}
	if v, _ := s.db.GetSetting("theme"); v != "ocean" {
		t.Fatalf("theme = %q, want ocean", v)
	}
}