// Package gateway 实现反向代理：把 /p/<插件名>/* 的请求转发到插件进程端口。
package gateway

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strings"

	"micropanel/internal/plugins"
)

type Gateway struct {
	mgr *plugins.Manager
}

func New(mgr *plugins.Manager) *Gateway {
	return &Gateway{mgr: mgr}
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

	target, _ := url.Parse(fmt.Sprintf("http://127.0.0.1:%d", rt.Port()))
	proxy := httputil.NewSingleHostReverseProxy(target)
	director := proxy.Director
	proxy.Director = func(req *http.Request) {
		director(req)
		req.URL.Path = pluginPath
		req.Host = target.Host
		req.Header.Set("X-Forwarded-Proto", "http")
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
