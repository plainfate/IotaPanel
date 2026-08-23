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

// Package api 提供面板 REST API 与前端页面路由。
package api

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io/fs"
	"log/slog"
	"net"
	"net/http"
	"path/filepath"
	"net/url"
	"strings"
	"time"

	"iotapanel/internal/auth"
	"iotapanel/internal/config"
	"iotapanel/internal/db"
	"iotapanel/internal/embed"
	"iotapanel/internal/gateway"
	"iotapanel/internal/plugins"
)

type ctxKey int

const sessionKey ctxKey = 0

type Server struct {
	cfg   *config.Config
	db    *db.DB
	mgr   *plugins.Manager
	gw    *gateway.Gateway
	prog  *setupProgress
	guard *loginGuard // 登录失败锁定
	start time.Time
	log   *slog.Logger
}

func NewServer(cfg *config.Config, database *db.DB, mgr *plugins.Manager, logger *slog.Logger) *Server {
	return &Server{
		cfg:   cfg,
		db:    database,
		mgr:   mgr,
		gw:    gateway.New(mgr, cfg.TrustProxy),
		prog:  &setupProgress{},
		guard: &loginGuard{fails: map[string]int{}, until: map[string]time.Time{}},
		start: time.Now(),
		log:   logger,
	}
}

func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/status", s.auth(s.handleStatus))
	mux.HandleFunc("POST /api/login", s.handleLogin)
	mux.HandleFunc("POST /api/logout", s.handleLogout)
	mux.HandleFunc("GET /api/me", s.auth(s.handleMe))
	mux.HandleFunc("GET /api/account", s.auth(s.handleAccount))
	mux.HandleFunc("POST /api/account/username", s.auth(s.handleUsernameChange))
	mux.HandleFunc("POST /api/account/password", s.auth(s.handleAccountPassword))
	mux.HandleFunc("GET /api/account/sessions", s.auth(s.handleSessionsList))
	mux.HandleFunc("POST /api/account/sessions/revoke", s.auth(s.handleSessionRevoke))
	mux.HandleFunc("POST /api/account/sessions/revoke-all", s.auth(s.handleSessionsRevokeAll))
	mux.HandleFunc("GET /api/security", s.auth(s.handleSecurityGet))
	mux.HandleFunc("PUT /api/security", s.auth(s.handleSecurityPut))
	mux.HandleFunc("GET /api/setup/state", s.handleSetupState)
	mux.HandleFunc("POST /api/setup/start", s.handleSetupStart)
	mux.HandleFunc("GET /api/setup/status", s.handleSetupStatus)
	mux.HandleFunc("GET /api/plugins", s.auth(s.handlePluginsList))
	mux.HandleFunc("POST /api/plugins/{name}/start", s.auth(s.handlePluginStart))
	mux.HandleFunc("POST /api/plugins/{name}/stop", s.auth(s.handlePluginStop))
	mux.HandleFunc("POST /api/plugins/{name}/restart", s.auth(s.handlePluginRestart))
	mux.HandleFunc("POST /api/plugins/{name}/keepalive", s.auth(s.handlePluginKeepalive))
	mux.HandleFunc("GET /api/plugins/{name}/log", s.auth(s.handlePluginLog))
	mux.HandleFunc("DELETE /api/plugins/{name}", s.auth(s.handlePluginDelete))
	mux.HandleFunc("GET /api/store", s.handleStoreList) // 未初始化时对向导开放
	mux.HandleFunc("POST /api/store/{name}/install", s.auth(s.handleStoreInstall))
	mux.HandleFunc("POST /api/store/install-url", s.auth(s.handleStoreInstallURL))
	mux.HandleFunc("GET /api/settings", s.auth(s.handleSettingsGet))
	mux.HandleFunc("PUT /api/settings", s.auth(s.handleSettingsPut))
	mux.HandleFunc("GET /api/log", s.auth(s.handleLog))
	mux.HandleFunc("POST /api/system/restart", s.auth(s.handleSystemRestart))
	// 插件网关：/p/<插件名>/* 支持任意方法（GET 页面、POST 插件 API 等）
	mux.Handle("/p/", http.HandlerFunc(s.handlePluginGateway))
	mux.HandleFunc("/", s.handleUI)
	// 顺序：安全响应头（最外层）→ 访问日志 → CSRF 校验 → 路由
	return s.logRequests(s.securityHeaders(s.csrfCheck(mux)))
}

// handlePluginGateway 插件网关入口：默认需面板登录；
// 插件 manifest 声明 auth: none 且路径为 /mcp 时免面板登录（由插件自身 Bearer 令牌鉴权，
// 供 MCP 客户端直连；页面/其它路径仍走面板登录）。
func (s *Server) handlePluginGateway(w http.ResponseWriter, r *http.Request) {
	rest := strings.TrimPrefix(r.URL.Path, "/p/")
	name, pluginPath, _ := strings.Cut(rest, "/")
	// 注意：strings.Cut 返回分隔符之后的部分（无前导斜杠），故比较 "mcp"
	if name != "" && pluginPath == "mcp" && s.pluginNoAuth(name) {
		s.gw.ServeHTTP(w, r)
		return
	}
	s.auth(http.HandlerFunc(s.gw.ServeHTTP))(w, r)
}

// pluginNoAuth 读取插件 manifest 判断是否声明 auth: none。
func (s *Server) pluginNoAuth(name string) bool {
	mf, err := plugins.LoadManifest(filepath.Join(s.cfg.Home, "plugins", name))
	return err == nil && mf.Auth == "none"
}

// securityHeaders 设置基础安全响应头：防点击劫持、防 MIME 嗅探，
// HTTPS（含受信反代透传）下启用 HSTS。
func (s *Server) securityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("X-Frame-Options", "SAMEORIGIN")
		h.Set("X-Content-Type-Options", "nosniff")
		if r.TLS != nil || (s.cfg.TrustProxy && strings.EqualFold(r.Header.Get("X-Forwarded-Proto"), "https")) {
			h.Set("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
		}
		next.ServeHTTP(w, r)
	})
}

// csrfCheck 对状态变更请求（POST/PUT/DELETE/PATCH）校验 Origin，
// 作为 SameSite=Lax 之外的纵深防御：Origin 缺失（非浏览器客户端如 curl）放行，
// 存在但与面板 Host 不同源则拒绝，阻断跨站请求伪造。
func (s *Server) csrfCheck(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPost, http.MethodPut, http.MethodDelete, http.MethodPatch:
			origin := r.Header.Get("Origin")
			if origin != "" && !sameOrigin(origin, r, s.cfg.TrustProxy) {
				writeJSON(w, http.StatusForbidden, map[string]string{"error": "跨站请求被拒绝"})
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

// sameOrigin 判断 Origin 头与请求目标是否同源（比较 host:port，忽略协议）。
// 仅当面板部署在受信反向代理之后（trustProxy）才采信 X-Forwarded-Host；
// 直连模式（默认）忽略该头——X-Forwarded-* 可被客户端伪造，一律以 r.Host 为准。
func sameOrigin(origin string, r *http.Request, trustProxy bool) bool {
	u, err := url.Parse(origin)
	if err != nil || u.Host == "" {
		return false
	}
	host := r.Host
	if trustProxy {
		if xh := r.Header.Get("X-Forwarded-Host"); xh != "" {
			host = xh
		}
	}
	return strings.EqualFold(u.Host, host)
}

// ---------- 中间件 ----------

func (s *Server) auth(h http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		c, err := r.Cookie(auth.CookieName)
		if err != nil {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "未登录"})
			return
		}
		sess, ok := auth.ParseToken(c.Value, []byte(s.cfg.JWTSecret))
		if !ok {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "会话无效或已过期"})
			return
		}
		// 会话持久化校验：令牌必须在 sessions 表中且未被强制下线
		rec, found, err := s.db.GetSessionByTokenHash(sha256Hex(c.Value))
		if err != nil || !found || rec.Revoked {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "会话已失效（可能已被强制下线）"})
			return
		}
		// 以数据库会话记录的用户名为准（支持修改用户名后会话仍有效）
		sess.Username = rec.Username
		h(w, r.WithContext(context.WithValue(r.Context(), sessionKey, sess)))
	}
}

// sessionFrom 从请求上下文取出认证中间件写入的会话信息。
func sessionFrom(r *http.Request) *auth.Session {
	if v, ok := r.Context().Value(sessionKey).(*auth.Session); ok {
		return v
	}
	return nil
}

func (s *Server) loggedIn(r *http.Request) bool {
	c, err := r.Cookie(auth.CookieName)
	if err != nil {
		return false
	}
	_, ok := auth.ParseToken(c.Value, []byte(s.cfg.JWTSecret))
	return ok
}

// logRequests 访问日志中间件：记录方法、路径、状态码与耗时。
func (s *Server) logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sw := &statusWriter{ResponseWriter: w, status: http.StatusOK}
		start := time.Now()
		next.ServeHTTP(sw, r)
		s.log.Info("http",
			"method", r.Method,
			"path", r.URL.Path,
			"status", sw.status,
			"ms", time.Since(start).Milliseconds(),
		)
	})
}

// statusWriter 包装 ResponseWriter 以捕获响应状态码。
type statusWriter struct {
	http.ResponseWriter
	status int
}

// WriteHeader 记录状态码并透传给下层。
func (sw *statusWriter) WriteHeader(code int) {
	sw.status = code
	sw.ResponseWriter.WriteHeader(code)
}

// Hijack 支持 WebSocket 升级（终端等插件需要）。
func (sw *statusWriter) Hijack() (net.Conn, *bufio.ReadWriter, error) {
	h, ok := sw.ResponseWriter.(http.Hijacker)
	if !ok {
		return nil, nil, errors.New("hijack not supported")
	}
	return h.Hijack()
}

// Flush 透传流式刷新（插件走网关的 SSE/长轮询响应需要）。
func (sw *statusWriter) Flush() {
	if f, ok := sw.ResponseWriter.(http.Flusher); ok {
		f.Flush()
	}
}

// ---------- 工具 ----------

// sha256Hex 计算字符串的 SHA-256 十六进制（会话令牌指纹）。
func sha256Hex(s string) string {
	sum := sha256.Sum256([]byte(s))
	return hex.EncodeToString(sum[:])
}

// writeJSON 统一输出 JSON 响应（设置 Content-Type + 状态码）。
func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

// serveFile 从内嵌资源中读取并输出一个前端页面文件。
func (s *Server) serveFile(w http.ResponseWriter, r *http.Request, name string) {
	data, err := fs.ReadFile(embed.Web, "web/"+name)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", mimeType(name))
	w.Write(data)
}

// mimeType 按扩展名返回 Content-Type（内嵌资源没有系统 MIME 推断）。
func mimeType(name string) string {
	switch {
	case len(name) > 5 && name[len(name)-5:] == ".html":
		return "text/html; charset=utf-8"
	case len(name) > 4 && name[len(name)-4:] == ".css":
		return "text/css; charset=utf-8"
	case len(name) > 3 && name[len(name)-3:] == ".js":
		return "application/javascript; charset=utf-8"
	case len(name) > 5 && name[len(name)-5:] == ".json":
		return "application/json; charset=utf-8"
	case len(name) > 4 && name[len(name)-4:] == ".ico":
		return "image/x-icon"
	default:
		return "application/octet-stream"
	}
}

// ---------- 前端页面路由 ----------

func (s *Server) handleUI(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.NotFound(w, r)
		return
	}
	configured, _ := s.db.HasAdmin()
	p := r.URL.Path
	switch p {
	case "/setup", "/setup/":
		if configured {
			http.Redirect(w, r, "/", http.StatusFound)
			return
		}
		s.serveFile(w, r, "setup.html")
	case "/login", "/login/":
		if !configured {
			http.Redirect(w, r, "/setup", http.StatusFound)
			return
		}
		if s.loggedIn(r) {
			http.Redirect(w, r, "/", http.StatusFound)
			return
		}
		s.serveFile(w, r, "login.html")
	case "/":
		if !configured {
			http.Redirect(w, r, "/setup", http.StatusFound)
			return
		}
		if !s.loggedIn(r) {
			http.Redirect(w, r, "/login", http.StatusFound)
			return
		}
		s.serveFile(w, r, "index.html")
	default:
		// 静态资源 /css/app.css 等
		sub, err := fs.Sub(embed.Web, "web")
		if err != nil {
			http.NotFound(w, r)
			return
		}
		http.FileServer(http.FS(sub)).ServeHTTP(w, r)
	}
}
