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

// HTTPS 网关插件：在面板前面终结 TLS（自签 / 已有证书 / Let's Encrypt ACME），
// 把流量反代回面板的 HTTP 端口，让面板无需内置 TLS 即可获得 HTTPS 访问。
//
// 通过面板侧边栏「HTTPS 网关」菜单进入配置页（插件端口 /p/https-front/），
// 保存配置后插件自动重启生效；TLS 启动失败时进程保持存活，方便改配置。
//
// 使用前提（写入 <安装目录>/etc/.env）：
//   LISTEN_ADDR=127.0.0.1:8787   # 面板只监听本机，本插件作为唯一对外入口
//   PANEL_TRUST_PROXY=1          # 信任本插件的 X-Forwarded-*（Secure cookie/HSTS/CSRF）
//
// 配置：<安装目录>/etc/https-front/config.yaml（配置页保存后由插件自动写回）
// 参考实现：Caddy / golang.org/x/crypto/acme/autocert 的 ACME 处理方式。
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"embed"
	"encoding/json"
	"encoding/pem"
	"errors"
	"log"
	"math/big"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"golang.org/x/crypto/acme/autocert"
	"gopkg.in/yaml.v3"
)

//go:embed web
var webFS embed.FS

// Config 插件配置（$PANEL_HOME/etc/https-front/config.yaml）。
type Config struct {
	PanelAddr    string `yaml:"panel_addr" json:"panel_addr"`       // 面板 HTTP 地址
	Listen       string `yaml:"listen" json:"listen"`               // 对外 HTTPS 监听，如 ":443" / ":8443"
	Mode         string `yaml:"mode" json:"mode"`                   // selfsigned | cert | acme
	CertFile     string `yaml:"cert_file" json:"cert_file"`         // mode=cert 时：证书链
	KeyFile      string `yaml:"key_file" json:"key_file"`           // mode=cert 时：私钥
	Domain       string `yaml:"domain" json:"domain"`               // mode=acme 时：域名（可逗号分隔多个）
	Email        string `yaml:"email" json:"email"`                 // mode=acme 时：联系邮箱（续期提醒）
	ACMEHTTPAddr string `yaml:"acme_http_addr" json:"acme_http_addr"` // mode=acme 时：HTTP-01 挑战监听（默认 :80）
}

const defaultConfigYAML = `# HTTPS 网关插件配置（配置页保存后自动重写本文件）
# 面板需设为仅本机监听并开启受信反代模式（etc/.env）：
#   LISTEN_ADDR=127.0.0.1:8787
#   PANEL_TRUST_PROXY=1
panel_addr: 127.0.0.1:8787
listen: ":8443"            # 对外 HTTPS 端口（想用 443 改这里；443 需要 root）
mode: selfsigned           # selfsigned | cert | acme
# ---- mode=cert：使用已有证书（如 certbot 签发）----
# cert_file: /etc/letsencrypt/live/example.com/fullchain.pem
# key_file:  /etc/letsencrypt/live/example.com/privkey.pem
# ---- mode=acme：Let's Encrypt 自动签发/续期 ----
# domain: example.com
# email: you@example.com
acme_http_addr: ":80"      # HTTP-01 挑战端口（需公网 80 可达且未被占用）
`

func main() {
	home := os.Getenv("PANEL_HOME")
	if home == "" {
		home = "/data/panel"
	}
	dir := filepath.Join(home, "etc", "https-front")
	os.MkdirAll(dir, 0o755)

	// 读取配置；不存在则生成默认配置
	cfgPath := filepath.Join(dir, "config.yaml")
	cfg := &Config{}
	if data, err := os.ReadFile(cfgPath); err == nil {
		_ = yaml.Unmarshal(data, cfg)
	} else if err := os.WriteFile(cfgPath, []byte(defaultConfigYAML), 0o600); err == nil {
		log.Printf("已生成默认配置: %s", cfgPath)
	}
	normalize(cfg)

	// ① 配置服务：监听面板分配的 PLUGIN_PORT（满足面板端口就绪探测），
	//    只提供配置页/API，绝不反代（防止 /p/https-front/ 网关环路）。
	port := os.Getenv("PLUGIN_PORT")
	if port == "" {
		port = "19005"
	}
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	statusSrv := &http.Server{Addr: bind + ":" + port, Handler: configHandler(home, dir, cfgPath), ReadHeaderTimeout: 10 * time.Second}
	go func() {
		log.Printf("[https-front] 配置服务就绪: %s（面板侧边栏「HTTPS 网关」可配置）", statusSrv.Addr)
		if err := statusSrv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Printf("[https-front] 配置服务异常: %v", err)
		}
	}()

	// ② TLS 反代服务：启动失败只记日志不退出（保持配置页可修复）
	tlsSrv := &http.Server{Addr: cfg.Listen, Handler: buildProxy(cfg), ReadHeaderTimeout: 10 * time.Second}
	switch cfg.Mode {
	case "acme":
		domains := splitDomains(cfg.Domain)
		mgr := &autocert.Manager{
			Prompt:     autocert.AcceptTOS,
			HostPolicy: autocert.HostWhitelist(domains...),
			Cache:      autocert.DirCache(filepath.Join(dir, "acme-cache")),
			Email:      cfg.Email,
		}
		go func() { // HTTP-01 挑战 + 自动跳转 HTTPS
			ch := &http.Server{Addr: cfg.ACMEHTTPAddr, Handler: mgr.HTTPHandler(nil), ReadHeaderTimeout: 10 * time.Second}
			log.Printf("[https-front] ACME 挑战监听: %s", ch.Addr)
			if err := ch.ListenAndServe(); err != nil && err != http.ErrServerClosed {
				log.Printf("[https-front] ACME 挑战服务异常: %v", err)
			}
		}()
		tlsSrv.TLSConfig = mgr.TLSConfig()
		log.Printf("[https-front] ACME 模式: 域名 %v，自动申请/续期 Let's Encrypt", domains)
		go func() {
			if err := tlsSrv.ListenAndServeTLS("", ""); err != nil && err != http.ErrServerClosed {
				log.Printf("[https-front] TLS 服务异常: %v（配置可能有误，可在配置页修改）", err)
			}
		}()
	case "cert":
		log.Printf("[https-front] 证书模式: %s", cfg.CertFile)
		go func() {
			if err := tlsSrv.ListenAndServeTLS(cfg.CertFile, cfg.KeyFile); err != nil && err != http.ErrServerClosed {
				log.Printf("[https-front] TLS 服务异常: %v（证书路径可能无效，可在配置页修改）", err)
			}
		}()
	default: // selfsigned
		certFile, keyFile := ensureSelfSigned(dir, cfg.Domain)
		log.Printf("[https-front] 自签证书模式（浏览器会有证书警告）: %s", certFile)
		go func() {
			if err := tlsSrv.ListenAndServeTLS(certFile, keyFile); err != nil && err != http.ErrServerClosed {
				log.Printf("[https-front] TLS 服务异常: %v", err)
			}
		}()
	}

	// 优雅退出
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
	<-sig
	log.Println("[https-front] 收到退出信号")
	_ = tlsSrv.Close()
	_ = statusSrv.Close()
}

// configHandler 配置页与 API：GET / 页面；GET/POST /api/config 读写配置；
// GET /api/status 运行状态与面板监听检查；POST /api/panel-setup 一键收紧面板监听。
// 仅监听插件端口（经面板网关 /p/https-front/* 访问）。
func configHandler(home, dir, cfgPath string) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /", func(w http.ResponseWriter, r *http.Request) {
		data, err := webFS.ReadFile("web/index.html")
		if err != nil {
			http.Error(w, "页面资源缺失", http.StatusInternalServerError)
			return
		}
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write(data)
	})
	mux.HandleFunc("GET /api/config", func(w http.ResponseWriter, r *http.Request) {
		cfg := &Config{}
		if data, err := os.ReadFile(cfgPath); err == nil {
			_ = yaml.Unmarshal(data, cfg)
		}
		normalize(cfg)
		writeJSON(w, http.StatusOK, cfg)
	})
	mux.HandleFunc("POST /api/config", func(w http.ResponseWriter, r *http.Request) {
		var cfg Config
		if err := json.NewDecoder(r.Body).Decode(&cfg); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误: " + err.Error()})
			return
		}
		normalize(&cfg)
		if err := validate(&cfg); err != nil {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": err.Error()})
			return
		}
		data, err := yaml.Marshal(&cfg)
		if err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
			return
		}
		if err := os.WriteFile(cfgPath, data, 0o600); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "写入配置失败: " + err.Error()})
			return
		}
		log.Printf("[https-front] 配置已保存（mode=%s listen=%s），请重启插件生效", cfg.Mode, cfg.Listen)
		writeJSON(w, http.StatusOK, map[string]any{"ok": true})
	})
	mux.HandleFunc("GET /api/status", func(w http.ResponseWriter, r *http.Request) {
		cfg := &Config{}
		if data, err := os.ReadFile(cfgPath); err == nil {
			_ = yaml.Unmarshal(data, cfg)
		}
		normalize(cfg)
		env := readPanelEnv(home)
		writeJSON(w, http.StatusOK, map[string]any{
			"mode":              cfg.Mode,
			"listen":            cfg.Listen,
			"panel_addr":        cfg.PanelAddr,
			"cert_expiry":       certExpiry(dir, cfg),
			"panel_listen":      env["LISTEN_ADDR"],
			"panel_trust_proxy": env["PANEL_TRUST_PROXY"] == "1",
			"panel_exposed":     panelExposed(cfg.PanelAddr),
		})
	})
	// 一键收紧：把面板监听改成仅本机 + 开启受信反代（写入面板 etc/.env，随后页面触发面板重启）
	mux.HandleFunc("POST /api/panel-setup", func(w http.ResponseWriter, r *http.Request) {
		cfg := &Config{}
		if data, err := os.ReadFile(cfgPath); err == nil {
			_ = yaml.Unmarshal(data, cfg)
		}
		normalize(cfg)
		host, port, err := net.SplitHostPort(cfg.PanelAddr)
		if err != nil || port == "" {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "面板地址格式无效（应为 host:port）"})
			return
		}
		_ = host // 一律收敛到回环，保证对外只有 HTTPS
		listenAddr := "127.0.0.1:" + port
		if err := setPanelEnv(home, map[string]string{
			"LISTEN_ADDR":      listenAddr,
			"PANEL_TRUST_PROXY": "1",
		}); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "写入面板配置失败: " + err.Error()})
			return
		}
		log.Printf("[https-front] 已写入面板 .env：LISTEN_ADDR=%s PANEL_TRUST_PROXY=1（需重启面板生效）", listenAddr)
		writeJSON(w, http.StatusOK, map[string]any{"ok": true, "listen_addr": listenAddr})
	})
	return mux
}

// normalize 补默认值；未知模式回退 selfsigned（不致命，保证配置页始终可用）。
func normalize(cfg *Config) {
	if cfg.PanelAddr == "" {
		cfg.PanelAddr = "127.0.0.1:8787"
	}
	if cfg.Listen == "" {
		cfg.Listen = ":8443"
	}
	if cfg.ACMEHTTPAddr == "" {
		cfg.ACMEHTTPAddr = ":80"
	}
	switch cfg.Mode {
	case "", "selfsigned":
		cfg.Mode = "selfsigned"
	case "cert", "acme":
	default:
		log.Printf("[https-front] 未知证书模式 %q，回退 selfsigned", cfg.Mode)
		cfg.Mode = "selfsigned"
	}
}

// validate 校验配置（配置页保存时调用）。
func validate(cfg *Config) error {
	switch cfg.Mode {
	case "selfsigned":
	case "cert":
		if cfg.CertFile == "" || cfg.KeyFile == "" {
			return errors.New("cert 模式需要填写证书文件与私钥文件")
		}
	case "acme":
		if cfg.Domain == "" {
			return errors.New("acme 模式需要填写域名")
		}
	default:
		return errors.New("未知证书模式: " + cfg.Mode)
	}
	return nil
}

// certExpiry 返回证书到期日期（自签或已配置证书），用于状态展示。
func certExpiry(dir string, cfg *Config) string {
	pemPath := filepath.Join(dir, "cert.pem")
	if cfg.Mode == "cert" && cfg.CertFile != "" {
		pemPath = cfg.CertFile
	}
	if b, err := os.ReadFile(pemPath); err == nil {
		if block, _ := pem.Decode(b); block != nil {
			if c, err := x509.ParseCertificate(block.Bytes); err == nil {
				return c.NotAfter.Format("2006-01-02")
			}
		}
	}
	return "-"
}

// buildProxy 反向代理到面板：保留原始 Host（面板 CSRF 与终端 WS 的 Origin 校验依赖），
// 并注入 X-Forwarded-Proto/Host（面板开 PANEL_TRUST_PROXY 后据此识别 HTTPS/原始域名）。
func buildProxy(cfg *Config) http.Handler {
	target, err := url.Parse("http://" + cfg.PanelAddr)
	if err != nil {
		log.Fatalf("panel_addr 无效: %v", err)
	}
	proxy := httputil.NewSingleHostReverseProxy(target)
	director := proxy.Director
	proxy.Director = func(req *http.Request) {
		origHost := req.Host // Director 执行前保存浏览器原始 Host
		director(req)        // 默认改写 URL 与 Host 到 target
		req.Host = origHost  // 还原，让面板看到真实域名
		req.Header.Set("X-Forwarded-Proto", "https")
		req.Header.Set("X-Forwarded-Host", origHost)
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		http.Error(w, "https-front 无法连接面板: "+err.Error(), http.StatusBadGateway)
	}
	return proxy
}

// ensureSelfSigned 生成并持久化自签 ECDSA 证书（首次运行），返回 cert/key 路径。
func ensureSelfSigned(dir, domain string) (string, string) {
	certFile := filepath.Join(dir, "cert.pem")
	keyFile := filepath.Join(dir, "key.pem")
	if _, err := os.Stat(certFile); err == nil {
		return certFile, keyFile
	}
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		log.Fatal(err)
	}
	serial, _ := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	tmpl := x509.Certificate{
		SerialNumber: serial,
		Subject:      pkix.Name{CommonName: "IotaPanel HTTPS"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(825 * 24 * time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	for _, d := range splitDomains(domain) {
		tmpl.DNSNames = append(tmpl.DNSNames, d)
	}
	der, err := x509.CreateCertificate(rand.Reader, &tmpl, &tmpl, &key.PublicKey, key)
	if err != nil {
		log.Fatal(err)
	}
	kb, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		log.Fatal(err)
	}
	os.WriteFile(certFile, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0o600)
	os.WriteFile(keyFile, pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: kb}), 0o600)
	return certFile, keyFile
}

func splitDomains(s string) []string {
	var out []string
	for _, d := range strings.Split(s, ",") {
		if d = strings.TrimSpace(d); d != "" {
			out = append(out, d)
		}
	}
	return out
}

// readPanelEnv 读取面板 etc/.env 的键值（供状态页展示监听/信任配置）。
func readPanelEnv(home string) map[string]string {
	out := map[string]string{}
	data, err := os.ReadFile(filepath.Join(home, "etc", ".env"))
	if err != nil {
		return out
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if k, v, ok := strings.Cut(line, "="); ok {
			out[strings.TrimSpace(k)] = strings.Trim(strings.TrimSpace(v), `"'`)
		}
	}
	return out
}

// setPanelEnv 更新面板 etc/.env 中的键，保留其他行与注释；文件不存在则创建。
func setPanelEnv(home string, kv map[string]string) error {
	path := filepath.Join(home, "etc", ".env")
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	var out []string
	updated := map[string]bool{}
	if data, err := os.ReadFile(path); err == nil {
		for _, ln := range strings.Split(string(data), "\n") {
			key := ""
			trimmed := strings.TrimSpace(ln)
			if !strings.HasPrefix(trimmed, "#") {
				if k, _, ok := strings.Cut(trimmed, "="); ok {
					key = strings.TrimSpace(k)
				}
			}
			if v, ok := kv[key]; ok {
				out = append(out, key+"="+v)
				updated[key] = true
				continue
			}
			out = append(out, ln)
		}
	}
	for k, v := range kv {
		if !updated[k] {
			out = append(out, k+"="+v)
		}
	}
	return os.WriteFile(path, []byte(strings.Join(out, "\n")+"\n"), 0o600)
}

// panelExposed 探测面板是否在非回环网卡上可达（HTTP 对外暴露）。
// 逐个尝试连接本机非回环 IPv4 地址上的面板端口，任一成功即判定暴露。
func panelExposed(panelAddr string) bool {
	_, port, err := net.SplitHostPort(panelAddr)
	if err != nil || port == "" {
		return false
	}
	addrs, err := net.InterfaceAddrs()
	if err != nil {
		return false
	}
	for _, a := range addrs {
		ipn, ok := a.(*net.IPNet)
		if !ok {
			continue
		}
		ip := ipn.IP.To4()
		if ip == nil || ip.IsLoopback() {
			continue
		}
		conn, err := net.DialTimeout("tcp", net.JoinHostPort(ip.String(), port), 500*time.Millisecond)
		if err == nil {
			conn.Close()
			return true
		}
	}
	return false
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

