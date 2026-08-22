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
	"net/http/httptest"
	"testing"
)

func TestSameOrigin(t *testing.T) {
	cases := []struct {
		origin     string
		host       string
		xfh        string // X-Forwarded-Host（仅 trustProxy=true 时采信）
		trustProxy bool
		want       bool
	}{
		{"http://127.0.0.1:8787", "127.0.0.1:8787", "", false, true},
		{"https://127.0.0.1:8787", "127.0.0.1:8787", "", false, true}, // 协议不同但 host 相同
		{"http://127.0.0.1:8788", "127.0.0.1:8787", "", false, false}, // 端口不同
		{"http://evil.example", "127.0.0.1:8787", "", false, false},   // 跨站
		// 直连模式：伪造 X-Forwarded-Host 也不采信（仍按 r.Host 判定）
		{"http://evil.example", "127.0.0.1:8787", "evil.example", false, false},
		{"http://127.0.0.1:8787", "127.0.0.1:8787", "evil.example", false, true},
		// 受信反代模式：以 X-Forwarded-Host（公网域名）为准
		{"http://panel.example.com", "10.0.0.5:8787", "panel.example.com", true, true},
		{"http://evil.example", "10.0.0.5:8787", "panel.example.com", true, false},
		{"not-a-url", "127.0.0.1:8787", "", false, false},
		{"", "127.0.0.1:8787", "", false, false},
	}
	for _, c := range cases {
		req := httptest.NewRequest("POST", "http://"+c.host+"/api/test", nil)
		if c.xfh != "" {
			req.Header.Set("X-Forwarded-Host", c.xfh)
		}
		if got := sameOrigin(c.origin, req, c.trustProxy); got != c.want {
			t.Errorf("sameOrigin(%q, host=%q, xfh=%q, trustProxy=%v) = %v, want %v",
				c.origin, c.host, c.xfh, c.trustProxy, got, c.want)
		}
	}
}
