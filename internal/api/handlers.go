package api

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"micropanel/internal/auth"
	"micropanel/internal/config"
	"micropanel/internal/db"
	"micropanel/internal/embed"
	"micropanel/internal/plugins"
)

// ---------- 首次初始化向导 ----------

type setupProgress struct {
	mu       sync.Mutex
	running  bool
	done     int
	total    int
	current  string
	complete bool
	errMsg   string
}

// snapshot 返回初始化进度的只读快照（轮询接口用）。
func (p *setupProgress) snapshot() map[string]any {
	p.mu.Lock()
	defer p.mu.Unlock()
	return map[string]any{
		"running":  p.running,
		"done":     p.done,
		"total":    p.total,
		"current":  p.current,
		"complete": p.complete,
		"error":    p.errMsg,
	}
}

// handleSetupState 返回面板是否已完成初始化。
func (s *Server) handleSetupState(w http.ResponseWriter, r *http.Request) {
	configured, _ := s.db.HasAdmin()
	writeJSON(w, http.StatusOK, map[string]any{"configured": configured})
}

// handleSetupStart 启动初始化：异步创建管理员并批量安装所选插件。
func (s *Server) handleSetupStart(w http.ResponseWriter, r *http.Request) {
	configured, _ := s.db.HasAdmin()
	if configured {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "面板已初始化"})
		return
	}
	var req struct {
		Username string   `json:"username"`
		Password string   `json:"password"`
		Plugins  []string `json:"plugins"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	if len(req.Username) < 3 || len(req.Password) < 6 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "用户名至少 3 位，密码至少 6 位"})
		return
	}
	s.prog.mu.Lock()
	if s.prog.running {
		s.prog.mu.Unlock()
		writeJSON(w, http.StatusConflict, map[string]string{"error": "初始化正在进行中"})
		return
	}
	s.prog.running, s.prog.done, s.prog.complete, s.prog.errMsg = true, 0, false, ""
	s.prog.total = 1 + len(req.Plugins)
	s.prog.mu.Unlock()

	go s.runSetup(req.Username, req.Password, req.Plugins)
	writeJSON(w, http.StatusAccepted, map[string]any{"ok": true})
}

// runSetup 执行初始化流程（在后台 goroutine 中运行，更新进度对象）。
func (s *Server) runSetup(username, password string, pluginNames []string) {
	step := func(name string) {
		s.prog.mu.Lock()
		s.prog.done++
		s.prog.current = name
		s.prog.mu.Unlock()
	}
	fail := func(errMsg string) {
		s.prog.mu.Lock()
		s.prog.running = false
		s.prog.errMsg = errMsg
		s.prog.mu.Unlock()
	}

	// 1. 创建管理员
	salt, hash, err := auth.HashPassword(password)
	if err != nil {
		fail("创建管理员失败: " + err.Error())
		return
	}
	if err := s.db.CreateUser(db.User{
		Username: username, PasswordHash: hash, Salt: salt, CreatedAt: db.Now(),
	}); err != nil {
		fail("创建管理员失败: " + err.Error())
		return
	}
	step("创建管理员账号")

	// 2. 批量安装所选插件
	for _, name := range pluginNames {
		if !plugins.CatalogContains(embed.Plugins, name) {
			continue
		}
		if err := s.installBundled(name); err != nil {
			fail("安装插件 " + name + " 失败: " + err.Error())
			return
		}
		step("安装 " + name)
	}

	s.prog.mu.Lock()
	s.prog.running = false
	s.prog.complete = true
	s.prog.current = "完成"
	s.prog.mu.Unlock()
}

// handleSetupStatus 返回初始化进度（前端进度条轮询）。
func (s *Server) handleSetupStatus(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.prog.snapshot())
}

// installBundled 安装内嵌官方插件（复制文件 + 登记数据库，无需重启面板）。
func (s *Server) installBundled(name string) error {
	if err := plugins.InstallFromEmbed(embed.Plugins, s.cfg.Home, name); err != nil {
		return err
	}
	mf, err := plugins.LoadManifest(filepath.Join(s.cfg.Home, "plugins", name))
	if err != nil {
		return err
	}
	if err := s.db.UpsertPlugin(db.PluginRecord{
		Name: mf.Name, Title: mf.Title, Version: mf.Version,
		Author: mf.Author, Description: mf.Description,
		InstalledAt: time.Now().Format(time.RFC3339), Source: "bundled",
	}); err != nil {
		return err
	}
	if mf.Keepalive {
		_ = s.db.SetKeepalive(name, true)
	}
	s.log.Info("plugin installed", "plugin", name, "version", mf.Version)
	return nil
}

// ---------- 认证 ----------

// handleLogin 校验用户名密码（含失败锁定），签发会话 cookie 并持久化会话记录。
// 支持「记住我」（30 天免登录）；单账号单会话：新登录自动踢掉该账号其他会话。
func (s *Server) handleLogin(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Username string `json:"username"`
		Password string `json:"password"`
		Remember bool   `json:"remember"` // 记住我：cookie 保留 30 天
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	configured, _ := s.db.HasAdmin()
	if !configured {
		writeJSON(w, http.StatusForbidden, map[string]string{"error": "面板尚未初始化"})
		return
	}

	// 登录失败锁定策略（设置页可调）
	failLimit, lockMin := s.securityPolicy()

	// 锁定检查
	if rem := s.guard.remaining(req.Username); rem > 0 {
		writeJSON(w, http.StatusLocked, map[string]string{
			"error": fmt.Sprintf("登录失败次数过多，账号已锁定，请 %d 分钟后再试", int(rem.Minutes())+1),
		})
		return
	}

	u, err := s.db.GetUserByName(req.Username)
	if err != nil || !auth.VerifyPassword(req.Password, u.Salt, u.PasswordHash) {
		locked, _ := s.guard.recordFail(req.Username, failLimit, lockMin)
		if locked {
			writeJSON(w, http.StatusUnauthorized, map[string]string{
				"error": fmt.Sprintf("密码错误次数过多，账号已锁定 %d 分钟", lockMin),
			})
			return
		}
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "用户名或密码错误"})
		return
	}
	s.guard.reset(req.Username)

	// 会话有效期：普通登录 24h；记住我 30 天
	ttl := 24 * time.Hour
	if req.Remember {
		ttl = 30 * 24 * time.Hour
	}

	// 签发会话：令牌 + 数据库持久化记录（可列表/强制下线）
	sess := auth.NewSession(u.ID, u.Username, ttl)
	token := sess.Token([]byte(s.cfg.JWTSecret))
	_ = s.db.CreateSession(db.Session{
		TokenHash: sha256Hex(token),
		JTI:       sess.JTI,
		Username:  u.Username,
		IP:        clientIP(r),
		UserAgent: truncate(r.UserAgent(), 200),
		CreatedAt: db.Now(),
		ExpiresAt: time.Unix(sess.Exp, 0).Format(time.RFC3339),
	})
	_ = s.db.UpdateLastLogin(u.Username)

	// 单账号单会话：新登录立即踢掉该账号的其他会话
	if revoked, _ := s.db.RevokeOtherSessions(u.Username, sess.JTI); revoked > 0 {
		s.log.Info("single-session: 踢掉旧会话", "username", u.Username, "revoked", revoked)
	}

	// 普通登录 = 会话级 cookie（关浏览器失效）；记住我 = 30 天持久 cookie
	cookie := &http.Cookie{
		Name: auth.CookieName, Value: token,
		Path: "/", HttpOnly: true, SameSite: http.SameSiteLaxMode,
	}
	if req.Remember {
		cookie.MaxAge = 30 * 24 * 3600
	}
	http.SetCookie(w, cookie)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "username": u.Username, "remember": req.Remember})
}

// handleLogout 清除 cookie 并吊销当前会话（强制下线自身）。
func (s *Server) handleLogout(w http.ResponseWriter, r *http.Request) {
	if sess := sessionFrom(r); sess != nil {
		_ = s.db.RevokeSessionByJTI(sess.JTI)
	}
	http.SetCookie(w, &http.Cookie{
		Name: auth.CookieName, Value: "", Path: "/", HttpOnly: true, MaxAge: -1,
	})
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// handleMe 返回当前登录用户信息。
func (s *Server) handleMe(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	writeJSON(w, http.StatusOK, map[string]any{"username": sess.Username, "uid": sess.UID})
}

// handleStatus 返回核心运行状态（版本、路径、插件统计等）。
func (s *Server) handleStatus(w http.ResponseWriter, r *http.Request) {
	records, _ := s.db.ListPlugins()
	running := 0
	for _, p := range records {
		if s.mgr.Status(p.Name).Running {
			running++
		}
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"version":              config.Version,
		"home":                 s.cfg.Home,
		"listen_addr":          s.cfg.ListenAddr,
		"uptime_seconds":       int(time.Since(s.start).Seconds()),
		"idle_timeout_minutes": int(s.cfg.IdleTimeout.Minutes()),
		"plugins_installed":    len(records),
		"plugins_running":      running,
	})
}

// ---------- 插件 ----------

// handlePluginsList 返回已安装插件列表（含菜单、运行状态、保活标记）。
func (s *Server) handlePluginsList(w http.ResponseWriter, r *http.Request) {
	records, err := s.db.ListPlugins()
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	type pluginView struct {
		Name        string         `json:"name"`
		Title       string         `json:"title"`
		Version     string         `json:"version"`
		Author      string         `json:"author"`
		Description string         `json:"description"`
		Keepalive   bool           `json:"keepalive"`
		Menus       []plugins.Menu `json:"menus"`
		Status      plugins.Status `json:"status"`
	}
	out := []pluginView{}
	for _, rec := range records {
		view := pluginView{
			Name: rec.Name, Title: rec.Title, Version: rec.Version,
			Author: rec.Author, Description: rec.Description, Keepalive: rec.Keepalive,
			Status: s.mgr.Status(rec.Name),
		}
		if mf, err := plugins.LoadManifest(filepath.Join(s.cfg.Home, "plugins", rec.Name)); err == nil {
			view.Menus = mf.Menus
		}
		out = append(out, view)
	}
	writeJSON(w, http.StatusOK, map[string]any{"plugins": out})
}

// handlePluginStart 冷启动指定插件进程。
func (s *Server) handlePluginStart(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	rt, err := s.mgr.Start(name)
	if err != nil {
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "port": rt.Port(), "pid": rt.PID()})
}

// handlePluginStop 停止指定插件进程并清理端口映射。
func (s *Server) handlePluginStop(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	if err := s.mgr.Stop(name); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// handlePluginRestart 重启指定插件（保活设置不受影响）。
func (s *Server) handlePluginRestart(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	if err := s.mgr.Restart(name); err != nil {
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// handlePluginKeepalive 切换插件的「后台保活」开关。
func (s *Server) handlePluginKeepalive(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	var req struct {
		Enabled bool `json:"enabled"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	if _, installed, _ := s.db.GetPlugin(name); !installed {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "插件未安装"})
		return
	}
	if err := s.db.SetKeepalive(name, req.Enabled); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	s.mgr.ApplyKeepalive(name, req.Enabled)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "keepalive": req.Enabled})
}

// handlePluginLog 返回指定插件的进程日志（logs/plugins/<name>.log）。
func (s *Server) handlePluginLog(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	path := filepath.Join(s.cfg.Home, "logs", "plugins", name+".log")
	data, err := os.ReadFile(path)
	if err != nil {
		writeJSON(w, http.StatusOK, map[string]string{"log": ""})
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"log": string(data)})
}

// handlePluginDelete 卸载插件：停止进程 → 删除目录 → 删除数据库记录。
func (s *Server) handlePluginDelete(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	if _, installed, _ := s.db.GetPlugin(name); !installed {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "插件未安装"})
		return
	}
	if err := s.mgr.Uninstall(name); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	if err := s.db.DeletePlugin(name); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// ---------- 商城 ----------

// handleStoreList 返回商城插件目录（未初始化时对初始化向导开放）。
func (s *Server) handleStoreList(w http.ResponseWriter, r *http.Request) {
	configured, _ := s.db.HasAdmin()
	if configured && !s.loggedIn(r) {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "未登录"})
		return
	}
	items, err := plugins.ListCatalog(embed.Plugins)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	for i := range items {
		_, installed, _ := s.db.GetPlugin(items[i].Name)
		items[i].Installed = installed
	}
	writeJSON(w, http.StatusOK, map[string]any{"store": items})
}

// handleStoreInstall 一键安装商城插件（无需重启面板）。
func (s *Server) handleStoreInstall(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	if !plugins.CatalogContains(embed.Plugins, name) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "商城不存在该插件"})
		return
	}
	if err := s.installBundled(name); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "plugin": name})
}

// ---------- 设置 ----------

// handleSettingsGet 返回面板设置与端口映射表。
func (s *Server) handleSettingsGet(w http.ResponseWriter, r *http.Request) {
	idleMin := int(s.cfg.IdleTimeout.Minutes())
	if v, ok := s.db.GetSetting("idle_timeout_minutes"); ok {
		if n, err := strconv.Atoi(v); err == nil {
			idleMin = n
		}
	}
	theme := "sage"
	if v, ok := s.db.GetSetting("theme"); ok && v != "" {
		theme = v
	}
	lang := ""
	if v, ok := s.db.GetSetting("lang"); ok && v != "" {
		lang = v
	}
	portMap := map[string]any{}
	if data, err := os.ReadFile(filepath.Join(s.cfg.Home, "etc", "port-map.json")); err == nil {
		_ = json.Unmarshal(data, &portMap)
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"version":              config.Version,
		"home":                 s.cfg.Home,
		"listen_addr":          s.cfg.ListenAddr,
		"idle_timeout_minutes": idleMin,
		"theme":                theme,
		"lang":                 lang,
		"port_map":             portMap,
	})
}

// 主题白名单（莫奈配色预设）
var themeNames = map[string]bool{"sage": true, "ocean": true, "rose": true, "lilac": true}

// handleSettingsPut 保存面板设置（空闲退出时间 / 主题 / 监听端口）。
func (s *Server) handleSettingsPut(w http.ResponseWriter, r *http.Request) {
	var req struct {
		IdleTimeoutMinutes int    `json:"idle_timeout_minutes"`
		Theme              string `json:"theme"`
		Lang               string `json:"lang"`
		ListenPort         int    `json:"listen_port"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	changed := false

	if req.IdleTimeoutMinutes > 0 {
		if req.IdleTimeoutMinutes < 1 || req.IdleTimeoutMinutes > 1440 {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "空闲退出时间需在 1-1440 分钟之间"})
			return
		}
		d := time.Duration(req.IdleTimeoutMinutes) * time.Minute
		s.cfg.IdleTimeout = d
		s.mgr.SetIdle(d)
		_ = s.db.SetSetting("idle_timeout_minutes", strconv.Itoa(req.IdleTimeoutMinutes))
		changed = true
	}

	if req.Theme != "" {
		if !themeNames[req.Theme] {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "未知主题: " + req.Theme})
			return
		}
		_ = s.db.SetSetting("theme", req.Theme)
		changed = true
	}

	if req.Lang != "" {
		if len(req.Lang) > 20 {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "语言标识过长"})
			return
		}
		_ = s.db.SetSetting("lang", req.Lang)
		changed = true
	}

	// 修改监听端口：写回 .env 的 LISTEN_ADDR，重启后生效
	needRestart := false
	if req.ListenPort > 0 {
		if req.ListenPort < 1 || req.ListenPort > 65535 {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "端口需在 1-65535 之间"})
			return
		}
		host := "0.0.0.0"
		if _, p, err := net.SplitHostPort(s.cfg.ListenAddr); err == nil {
			host = strings.TrimSuffix(s.cfg.ListenAddr, ":"+p)
			if host == "" {
				host = "0.0.0.0"
			}
		}
		newAddr := net.JoinHostPort(host, strconv.Itoa(req.ListenPort))
		if err := config.SetEnvVar(s.cfg.Home, "LISTEN_ADDR", newAddr); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "写入配置失败: " + err.Error()})
			return
		}
		s.cfg.ListenAddr = newAddr
		needRestart = true
		changed = true
	}

	if !changed {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "没有可保存的设置项"})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "need_restart": needRestart})
}
