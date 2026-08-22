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

package api

import (
	"net/http/httptest"
	"testing"
)

func TestSameOrigin(t *testing.T) {
	cases := []struct {
		origin string
		host   string
		xfh    string // X-Forwarded-Host（反向代理部署）
		want   bool
	}{
		{"http://127.0.0.1:8787", "127.0.0.1:8787", "", true},
		{"https://127.0.0.1:8787", "127.0.0.1:8787", "", true}, // 协议不同但 host 相同
		{"http://127.0.0.1:8788", "127.0.0.1:8787", "", false}, // 端口不同
		{"http://evil.example", "127.0.0.1:8787", "", false},   // 跨站
		{"http://panel.example.com", "10.0.0.5:8787", "panel.example.com", true}, // 反代：Origin 是公网域名
		{"http://evil.example", "10.0.0.5:8787", "panel.example.com", false},
		{"not-a-url", "127.0.0.1:8787", "", false},
		{"", "127.0.0.1:8787", "", false},
	}
	for _, c := range cases {
		req := httptest.NewRequest("POST", "http://"+c.host+"/api/test", nil)
		if c.xfh != "" {
			req.Header.Set("X-Forwarded-Host", c.xfh)
		}
		if got := sameOrigin(c.origin, req); got != c.want {
			t.Errorf("sameOrigin(%q, host=%q, xfh=%q) = %v, want %v", c.origin, c.host, c.xfh, got, c.want)
		}
	}
}
