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

// mcp-agent：MCP 服务器插件（读 + 写）。
// AI 客户端经 HTTP POST /mcp（JSON-RPC 2.0，Authorization: Bearer <token>）连接。
// 工具：
//   只读：get_status / list_plugins / get_logs / get_metrics
//   写操作（需面板管理员凭据，见 config.yaml）：plugin_action（start/stop/restart/keepalive）
//   高危（默认关闭）：run_command（config.yaml 设 allow_shell: true 才可用）
package main

import (
	"bytes"
	"crypto/rand"
	"embed"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"

	"gopkg.in/yaml.v3"
)

//go:embed web
var webFS embed.FS

type Manifest struct {
	Name    string `yaml:"name"`
	Title   string `yaml:"title"`
	Version string `yaml:"version"`
}

// Config 插件配置（$PANEL_HOME/etc/mcp-agent/config.yaml）。
type Config struct {
	PanelAddr     string `yaml:"panel_addr"`     // 面板地址（写操作调用其 API）
	AdminUser     string `yaml:"admin_user"`     // 面板管理员用户名
	AdminPassword string `yaml:"admin_password"` // 面板管理员密码（写操作需要）
	AllowShell    bool   `yaml:"allow_shell"`    // 是否开放 run_command（高危，默认关）
	EnableRead    bool   `yaml:"enable_read"`    // 只读工具开关（默认开）
	EnableWrite   bool   `yaml:"enable_write"`   // 写操作 plugin_action 开关（默认开）
}

const defaultConfig = `# mcp-agent 配置（写操作需要面板管理员凭据）
panel_addr: 127.0.0.1:8787
admin_user: admin
admin_password: ""
enable_read: true
enable_write: true
# 高危：设为 true 后开放 run_command（AI 可直接执行 shell 命令）
allow_shell: false
`

func main() {
	home := os.Getenv("PANEL_HOME")
	if home == "" {
		home = "/data/panel"
	}
	dir := filepath.Join(home, "etc", "mcp-agent")
	os.MkdirAll(dir, 0o755)
	token := loadToken(dir)
	cfg := loadConfig(dir)

	pc := &panelClient{addr: cfg.PanelAddr, user: cfg.AdminUser, pass: cfg.AdminPassword, client: &http.Client{Timeout: 10 * time.Second}}

	port := os.Getenv("PLUGIN_PORT")
	if port == "" {
		port = "19006"
	}
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	mux := http.NewServeMux()
	mux.HandleFunc("GET /", pageHandler(token))
	mux.HandleFunc("POST /mcp", func(w http.ResponseWriter, r *http.Request) { mcpHandler(w, r, token, home, cfg, pc) })
	mux.HandleFunc("GET /api/config", func(w http.ResponseWriter, r *http.Request) { writeJSON(w, http.StatusOK, cfg) })
	mux.HandleFunc("POST /api/config", func(w http.ResponseWriter, r *http.Request) {
		var c Config
		if err := json.NewDecoder(r.Body).Decode(&c); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
			return
		}
		data, err := yaml.Marshal(&c)
		if err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
			return
		}
		if err := os.WriteFile(filepath.Join(dir, "config.yaml"), data, 0o600); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "写入失败: " + err.Error()})
			return
		}
		log.Printf("[mcp-agent] 配置已保存，重启插件生效")
		writeJSON(w, http.StatusOK, map[string]any{"ok": true})
	})
	srv := &http.Server{Addr: bind + ":" + port, Handler: mux, ReadHeaderTimeout: 10 * time.Second}
	go func() {
		log.Printf("[mcp-agent] 就绪: %s（侧边栏「MCP Agent」查看连接信息；写操作可用: %v）", srv.Addr, cfg.AdminPassword != "")
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Printf("[mcp-agent] 服务异常: %v", err)
		}
	}()
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
	<-sig
	_ = srv.Close()
}

func loadToken(dir string) string {
	p := filepath.Join(dir, "token")
	if b, err := os.ReadFile(p); err == nil && len(strings.TrimSpace(string(b))) >= 16 {
		return strings.TrimSpace(string(b))
	}
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	tok := hex.EncodeToString(b)
	_ = os.WriteFile(p, []byte(tok+"\n"), 0o600)
	log.Printf("[mcp-agent] 已生成访问令牌: %s（Authorization: Bearer %s）", p, tok)
	return tok
}

func loadConfig(dir string) Config {
	cfg := Config{PanelAddr: "127.0.0.1:8787"}
	p := filepath.Join(dir, "config.yaml")
	if b, err := os.ReadFile(p); err == nil {
		_ = yaml.Unmarshal(b, &cfg)
	} else {
		_ = os.WriteFile(p, []byte(defaultConfig), 0o600)
		_ = yaml.Unmarshal([]byte(defaultConfig), &cfg) // 首次生成也要反解默认值，否则开关为零值
		log.Printf("[mcp-agent] 已生成默认配置: %s（填 admin_password 启用写操作）", p)
	}
	if cfg.PanelAddr == "" {
		cfg.PanelAddr = "127.0.0.1:8787"
	}
	// 兼容旧配置：文件里没有开关字段时默认开启（避免升级后工具全关）
	raw, _ := os.ReadFile(p)
	if !bytes.Contains(raw, []byte("enable_read")) {
		cfg.EnableRead = true
	}
	if !bytes.Contains(raw, []byte("enable_write")) {
		cfg.EnableWrite = true
	}
	return cfg
}

func pageHandler(token string) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		data, err := webFS.ReadFile("web/index.html")
		if err != nil {
			http.Error(w, "页面缺失", http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write([]byte(strings.ReplaceAll(string(data), "__TOKEN__", token)))
	}
}

// ---- 面板 API 客户端（写操作）----

type panelClient struct {
	addr, user, pass, cookie string
	client                   *http.Client
}

func (p *panelClient) login() error {
	body, _ := json.Marshal(map[string]any{"username": p.user, "password": p.pass})
	resp, err := p.client.Post("http://"+p.addr+"/api/login", "application/json", bytes.NewReader(body))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, resp.Body)
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("面板登录失败 HTTP %d（检查 admin_user/admin_password）", resp.StatusCode)
	}
	p.cookie = ""
	for _, c := range resp.Cookies() {
		if c.Name == "mp_session" {
			p.cookie = c.Value
		}
	}
	if p.cookie == "" {
		return fmt.Errorf("面板未返回会话 cookie")
	}
	return nil
}

// api 调用面板 API；401 时自动重新登录重试一次。
func (p *panelClient) api(method, path, body string) (int, []byte, error) {
	do := func() (int, []byte, error) {
		var rd io.Reader
		if body != "" {
			rd = strings.NewReader(body)
		}
		req, err := http.NewRequest(method, "http://"+p.addr+path, rd)
		if err != nil {
			return 0, nil, err
		}
		if p.cookie != "" {
			req.Header.Set("Cookie", "mp_session="+p.cookie)
		}
		resp, err := p.client.Do(req)
		if err != nil {
			return 0, nil, err
		}
		defer resp.Body.Close()
		b, _ := io.ReadAll(resp.Body)
		return resp.StatusCode, b, nil
	}
	code, b, err := do()
	if err == nil && code == http.StatusUnauthorized && p.pass != "" {
		if lerr := p.login(); lerr == nil {
			return do()
		}
	}
	return code, b, err
}

// ---- MCP ----

func toolDefs() []map[string]any {
	return []map[string]any{
		{"name": "get_status", "description": "面板总体状态：管理员、插件数、运行中插件、监听配置", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}},
		{"name": "list_plugins", "description": "列出已安装插件（名称/标题/版本/是否运行）", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}},
		{"name": "get_logs", "description": "读取面板核心日志末尾（lines 默认 80）", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{"lines": map[string]any{"type": "number"}}}},
		{"name": "get_metrics", "description": "系统资源：CPU 负载、内存、磁盘、进程数", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{}}},
		{"name": "plugin_action", "description": "控制插件进程：action=start|stop|restart|keepalive，keepalive 需 enabled 布尔", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{"plugin": map[string]any{"type": "string"}, "action": map[string]any{"type": "string", "enum": []string{"start", "stop", "restart", "keepalive"}}, "enabled": map[string]any{"type": "boolean"}}}},
		{"name": "run_command", "description": "执行 shell 命令（需 config.yaml 设 allow_shell: true；高危）", "inputSchema": map[string]any{"type": "object", "properties": map[string]any{"command": map[string]any{"type": "string"}}}},
	}
}

func mcpHandler(w http.ResponseWriter, r *http.Request, token, home string, cfg Config, pc *panelClient) {
	if r.Header.Get("Authorization") != "Bearer "+token {
		writeJSON(w, http.StatusUnauthorized, map[string]any{"jsonrpc": "2.0", "error": map[string]any{"code": -32001, "message": "无效的访问令牌"}})
		return
	}
	var req struct {
		ID     any            `json:"id"`
		Method string         `json:"method"`
		Params map[string]any `json:"params"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]any{"jsonrpc": "2.0", "error": map[string]any{"code": -32700, "message": "解析失败"}})
		return
	}
	resp := map[string]any{"jsonrpc": "2.0"}
	switch req.Method {
	case "initialize":
		resp["result"] = map[string]any{"protocolVersion": "2024-11-05", "capabilities": map[string]any{"tools": map[string]any{}}, "serverInfo": map[string]any{"name": "iotapanel-mcp", "version": "0.2.0"}}
	case "notifications/initialized", "notifications/cancelled", "shutdown":
		if req.ID == nil {
			return
		}
		resp["result"] = map[string]any{}
	case "ping":
		resp["result"] = map[string]any{}
	case "tools/list":
		resp["result"] = map[string]any{"tools": toolDefs()}
	case "tools/call":
		name, _ := req.Params["name"].(string)
		args, _ := req.Params["arguments"].(map[string]any)
		text, err := callTool(name, args, home, cfg, pc)
		if err != nil {
			resp["error"] = map[string]any{"code": -32602, "message": err.Error()}
		} else {
			resp["result"] = map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": false}
		}
	default:
		resp["error"] = map[string]any{"code": -32601, "message": "未知方法: " + req.Method}
	}
	if req.ID != nil {
		resp["id"] = req.ID
		writeJSON(w, http.StatusOK, resp)
	}
}

func callTool(name string, args map[string]any, home string, cfg Config, pc *panelClient) (string, error) {
	readTools := map[string]bool{"get_status": true, "list_plugins": true, "get_logs": true, "get_metrics": true}
	if readTools[name] && !cfg.EnableRead {
		return "", fmt.Errorf("只读工具已关闭（可在配置页开启）")
	}
	if name == "plugin_action" && !cfg.EnableWrite {
		return "", fmt.Errorf("写操作已关闭（可在配置页开启）")
	}
	switch name {
	case "get_status":
		var users, plugins, running int
		if data, err := os.ReadFile(filepath.Join(home, "data", "panel.json")); err == nil {
			var d struct {
				Users   []any `json:"users"`
				Plugins []any `json:"plugins"`
			}
			_ = json.Unmarshal(data, &d)
			users, plugins = len(d.Users), len(d.Plugins)
		}
		if data, err := os.ReadFile(filepath.Join(home, "etc", "port-map.json")); err == nil {
			var m map[string]any
			if json.Unmarshal(data, &m) == nil {
				running = len(m)
			}
		}
		env := readEnv(filepath.Join(home, "etc", ".env"))
		return fmt.Sprintf("panel_home=%s\nlisten_addr=%s\nadmin_created=%v\nplugins_installed=%d\nplugins_running=%d\ntrust_proxy=%s",
			home, env["LISTEN_ADDR"], users > 0, plugins, running, env["PANEL_TRUST_PROXY"]), nil
	case "list_plugins":
		entries, err := os.ReadDir(filepath.Join(home, "plugins"))
		if err != nil {
			return "", err
		}
		var pm map[string]any
		if data, err := os.ReadFile(filepath.Join(home, "etc", "port-map.json")); err == nil {
			_ = json.Unmarshal(data, &pm)
		}
		var out []string
		for _, e := range entries {
			if !e.IsDir() {
				continue
			}
			mf := Manifest{}
			if b, err := os.ReadFile(filepath.Join(home, "plugins", e.Name(), "manifest.yaml")); err == nil {
				_ = yaml.Unmarshal(b, &mf)
			}
			_, running := pm[e.Name()]
			out = append(out, fmt.Sprintf("- %s (%s v%s) running=%v", e.Name(), mf.Title, mf.Version, running))
		}
		sort.Strings(out)
		return strings.Join(out, "\n"), nil
	case "get_logs":
		lines := 80
		if n, ok := args["lines"].(float64); ok && n > 0 {
			lines = int(n)
		}
		data, err := os.ReadFile(filepath.Join(home, "logs", "panel.log"))
		if err != nil {
			return "", err
		}
		all := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
		if len(all) > lines {
			all = all[len(all)-lines:]
		}
		return strings.Join(all, "\n"), nil
	case "get_metrics":
		out, err := exec.Command("sh", "-c", "uptime; free -m | head -2; df -h / | tail -1; echo processes=$(ps -e --no-headers | wc -l)").CombinedOutput()
		if err != nil {
			return string(out), err
		}
		return string(out), nil
	case "plugin_action":
		if cfg.AdminPassword == "" {
			return "", fmt.Errorf("写操作未启用：请先在 %s 填写 admin_password", filepath.Join(home, "etc", "mcp-agent", "config.yaml"))
		}
		plugin, _ := args["plugin"].(string)
		action, _ := args["action"].(string)
		if plugin == "" {
			return "", fmt.Errorf("缺少 plugin")
		}
		var body string
		switch action {
		case "start", "stop", "restart":
		case "keepalive":
			enabled, _ := args["enabled"].(bool)
			b, _ := json.Marshal(map[string]any{"enabled": enabled})
			body = string(b)
		default:
			return "", fmt.Errorf("action 需为 start/stop/restart/keepalive")
		}
		code, b, err := pc.api("POST", "/api/plugins/"+url.PathEscape(plugin)+"/"+action, body)
		if err != nil {
			return "", err
		}
		if code != http.StatusOK {
			return "", fmt.Errorf("面板返回 HTTP %d: %s", code, strings.TrimSpace(string(b)))
		}
		return strings.TrimSpace(string(b)), nil
	case "run_command":
		if !cfg.AllowShell {
			return "", fmt.Errorf("run_command 未启用：config.yaml 设 allow_shell: true（高危操作请谨慎）")
		}
		cmd, _ := args["command"].(string)
		if cmd == "" {
			return "", fmt.Errorf("缺少 command")
		}
		out, err := exec.Command("sh", "-c", cmd).CombinedOutput()
		text := string(out)
		if err != nil {
			text += fmt.Sprintf("\n[exit %v]", err)
		}
		return text, nil
	}
	return "", fmt.Errorf("未知工具: %s", name)
}

func readEnv(p string) map[string]string {
	out := map[string]string{}
	if b, err := os.ReadFile(p); err == nil {
		for _, ln := range strings.Split(string(b), "\n") {
			ln = strings.TrimSpace(ln)
			if ln == "" || strings.HasPrefix(ln, "#") {
				continue
			}
			if k, v, ok := strings.Cut(ln, "="); ok {
				out[strings.TrimSpace(k)] = strings.Trim(strings.TrimSpace(v), `"'`)
			}
		}
	}
	return out
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(code)
	json.NewEncoder(w).Encode(v)
}

func envOr(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

var _ = time.Second
