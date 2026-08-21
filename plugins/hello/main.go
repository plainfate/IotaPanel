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

// Hello 演示插件（Go 版）。
//
// 说明：最初版本用 Shell + Python3（python3 -m http.server）实现，
// 常驻内存约 37MB；改为 Go 后约 7MB，体现"极简保活"。
// 插件仍可任意语言编写（manifest.command 指向任何可执行文件/脚本）。
//
// 本插件同时演示面板核心注入的环境变量（PLUGIN_PORT / PANEL_HOME 等）。
package main

import (
	"embed"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
)

//go:embed web
var webFS embed.FS

func main() {
	port := os.Getenv("PLUGIN_PORT")
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	if port == "" {
		port = "19003" // 手动运行时使用默认端口
	}

	mux := http.NewServeMux()
	mux.HandleFunc("GET /", handleIndex)
	mux.HandleFunc("GET /api/info", handleInfo) // 展示面板注入的环境变量

	addr := bind + ":" + port
	server := &http.Server{Addr: addr, Handler: mux}
	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		server.Close()
	}()
	log.Printf("[hello] listening on %s", addr)
	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}

// handleIndex 返回内嵌演示页面。
func handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	data, err := webFS.ReadFile("web/index.html")
	if err != nil {
		http.Error(w, "页面资源缺失", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Write(data)
}

// handleInfo 返回核心注入的环境变量（证明插件与核心通过环境变量通信）。
func handleInfo(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, map[string]string{
		"plugin_port":        os.Getenv("PLUGIN_PORT"),
		"plugin_bind":        os.Getenv("PLUGIN_BIND"),
		"plugin_name":        os.Getenv("PLUGIN_NAME"),
		"panel_home":         os.Getenv("PANEL_HOME"),
		"micropanel_version": os.Getenv("MICROPANEL_VERSION"),
	})
}

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	json.NewEncoder(w).Encode(v)
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
