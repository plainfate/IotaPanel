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

// Package gateway 实现反向代理：把 /p/<插件名>/* 的请求转发到插件进程端口。
package gateway

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"

	"iotapanel/internal/plugins"
)

type Gateway struct {
	mgr        *plugins.Manager
	trustProxy bool
}

func New(mgr *plugins.Manager, trustProxy bool) *Gateway {
	return &Gateway{mgr: mgr, trustProxy: trustProxy}
}

func (g *Gateway) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	rest := strings.TrimPrefix(r.URL.Path, "/p/")
	name, pluginPath, _ := strings.Cut(rest, "/")
	if name == "" {
		http.Error(w, "missing plugin name", http.StatusNotFound)
		return
	}
	if !strings.HasPrefix(pluginPath, "/") {
		pluginPath = "/" + pluginPath
	}

	rt, err := g.mgr.Start(name) // 冷启动：若进程未运行则同步拉起（约 1-2 秒）
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	g.mgr.Touch(name)

	// 按插件 manifest.bind 连接（支持 0.0.0.0 / 指定网卡 IP 的插件）
	target, _ := url.Parse(fmt.Sprintf("http://%s:%d", rt.Bind(), rt.Port()))
	proxy := httputil.NewSingleHostReverseProxy(target)
	origHost := r.Host
	// 协议透传：仅受信反代模式下沿用入站 X-Forwarded-Proto（否则客户端可伪造），
	// 其余按连接本身推断（TLS/https，否则 http）
	proto := ""
	if g.trustProxy {
		proto = r.Header.Get("X-Forwarded-Proto")
	}
	if proto == "" {
		if r.TLS != nil {
			proto = "https"
		} else {
			proto = "http"
		}
	}
	director := proxy.Director
	proxy.Director = func(req *http.Request) {
		director(req)
		req.URL.Path = pluginPath
		req.Host = target.Host
		req.Header.Set("X-Forwarded-Proto", proto)
		req.Header.Set("X-Forwarded-Host", origHost)
		req.Header.Set("X-Panel-Plugin", name)
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		writeError(w, http.StatusBadGateway, "插件连接失败: "+err.Error())
	}
	proxy.ServeHTTP(w, r)
}

// writeError 以 JSON 形式输出网关错误（插件启动失败 / 连接失败等）。
func writeError(w http.ResponseWriter, code int, msg string) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(map[string]string{"error": msg})
}
