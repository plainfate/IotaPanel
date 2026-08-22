// SPDX-License-Identifier: GPL-3.0-or-later
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// IotaPanel is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// IotaPanel is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with IotaPanel.  If not, see <https://www.gnu.org/licenses/>.

// HTTPS 网关插件：在面板前面终结 TLS（自签 / 已有证书 / Let's Encrypt ACME），
// 把流量反代回面板的 HTTP 端口，让面板无需内置 TLS 即可获得 HTTPS 访问。
//
// 使用前提（写入 <安装目录>/etc/.env）：
//   LISTEN_ADDR=127.0.0.1:8787   # 面板只监听本机，本插件作为唯一对外入口
//   PANEL_TRUST_PROXY=1          # 信任本插件的 X-Forwarded-*（Secure cookie/HSTS/CSRF）
//
// 配置：<安装目录>/etc/https-front/config.yaml（首次运行自动生成默认配置）
// 参考实现：Caddy / golang.org/x/crypto/acme/autocert 的 ACME 处理方式。
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"log"
	"math/big"
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

// Config 插件配置（$PANEL_HOME/etc/https-front/config.yaml）。
type Config struct {
	PanelAddr    string `yaml:"panel_addr"`     // 面板 HTTP 地址
	Listen       string `yaml:"listen"`         // 对外 HTTPS 监听，如 ":443" / ":8443"
	Mode         string `yaml:"mode"`           // selfsigned | cert | acme
	CertFile     string `yaml:"cert_file"`      // mode=cert 时：证书链
	KeyFile      string `yaml:"key_file"`       // mode=cert 时：私钥
	Domain       string `yaml:"domain"`         // mode=acme 时：域名（可逗号分隔多个）
	Email        string `yaml:"email"`          // mode=acme 时：联系邮箱（续期提醒）
	ACMEHTTPAddr string `yaml:"acme_http_addr"` // mode=acme 时：HTTP-01 挑战监听（默认 :80）
}

const defaultConfigYAML = `# HTTPS 网关插件配置（修改后重启插件生效）
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
		log.Printf("已生成默认配置: %s（请按需修改后重启插件）", cfgPath)
	}
	normalize(cfg)

	// ① 状态页服务：监听面板分配的 PLUGIN_PORT（满足面板端口就绪探测；仅返回状态，绝不反代，防环路）
	port := os.Getenv("PLUGIN_PORT")
	if port == "" {
		port = "19005"
	}
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	statusSrv := &http.Server{Addr: bind + ":" + port, Handler: statusHandler(dir), ReadHeaderTimeout: 10 * time.Second}
	go func() {
		log.Printf("[https-front] status 端口就绪: %s", statusSrv.Addr)
		if err := statusSrv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatal(err)
		}
	}()

	// ② TLS 反代服务
	proxy := buildProxy(cfg)
	tlsSrv := &http.Server{Addr: cfg.Listen, Handler: proxy, ReadHeaderTimeout: 10 * time.Second}
	httpError := make(chan error, 1)
	switch cfg.Mode {
	case "acme":
		domains := strings.Split(cfg.Domain, ",")
		for i := range domains {
			domains[i] = strings.TrimSpace(domains[i])
		}
		mgr := &autocert.Manager{
			Prompt:     autocert.AcceptTOS,
			HostPolicy: autocert.HostWhitelist(domains...),
			Cache:      autocert.DirCache(filepath.Join(dir, "acme-cache")),
			Email:      cfg.Email,
		}
		// HTTP-01 挑战 + 自动跳转 HTTPS
		go func() {
			ch := &http.Server{Addr: cfg.ACMEHTTPAddr, Handler: mgr.HTTPHandler(nil), ReadHeaderTimeout: 10 * time.Second}
			log.Printf("[https-front] ACME 挑战监听: %s", ch.Addr)
			httpError <- ch.ListenAndServe()
		}()
		tlsSrv.TLSConfig = mgr.TLSConfig()
		log.Printf("[https-front] ACME 模式: 域名 %v，自动申请/续期 Let's Encrypt", domains)
		go func() { httpError <- tlsSrv.ListenAndServeTLS("", "") }()
	case "cert":
		log.Printf("[https-front] 证书模式: %s", cfg.CertFile)
		go func() { httpError <- tlsSrv.ListenAndServeTLS(cfg.CertFile, cfg.KeyFile) }()
	default: // selfsigned
		certFile, keyFile := ensureSelfSigned(dir, cfg.Domain)
		log.Printf("[https-front] 自签证书模式（浏览器会有证书警告）: %s", certFile)
		go func() { httpError <- tlsSrv.ListenAndServeTLS(certFile, keyFile) }()
	}

	// 优雅退出
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
	select {
	case <-sig:
		log.Println("[https-front] 收到退出信号")
		_ = tlsSrv.Close()
		_ = statusSrv.Close()
	case err := <-httpError:
		log.Fatalf("[https-front] TLS 服务异常退出: %v", err)
	}
}

// normalize 补默认值并校验配置。
func normalize(cfg *Config) {
	if cfg.PanelAddr == "" {
		cfg.PanelAddr = "127.0.0.1:8787"
	}
	if cfg.Listen == "" {
		cfg.Listen = ":8443"
	}
	if cfg.Mode == "" {
		cfg.Mode = "selfsigned"
	}
	if cfg.ACMEHTTPAddr == "" {
		cfg.ACMEHTTPAddr = ":80"
	}
	switch cfg.Mode {
	case "selfsigned", "cert", "acme":
	default:
		log.Fatalf("未知证书模式: %s（支持 selfsigned/cert/acme）", cfg.Mode)
	}
	if cfg.Mode == "cert" && (cfg.CertFile == "" || cfg.KeyFile == "") {
		log.Fatal("mode=cert 需要配置 cert_file 与 key_file")
	}
	if cfg.Mode == "acme" && cfg.Domain == "" {
		log.Fatal("mode=acme 需要配置 domain")
	}
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

// statusHandler 返回插件状态页（仅通过面板网关 /p/https-front/ 或直连插件端口访问）。
func statusHandler(dir string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		expiry := "（自签证书，无过期信息）"
		if b, err := os.ReadFile(filepath.Join(dir, "cert.pem")); err == nil {
			if block, _ := pem.Decode(b); block != nil {
				if c, err := x509.ParseCertificate(block.Bytes); err == nil {
					expiry = c.NotAfter.Format("2006-01-02")
				}
			}
		}
		fmt.Fprintf(w, `<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8"><title>HTTPS 网关</title></head>
<body style="font-family:ui-monospace,monospace;max-width:640px;margin:40px auto">
<h2>HTTPS 网关</h2>
<p>面板入口已由本插件提供 HTTPS 终结（TLS 反代到面板）。</p>
<p>证书到期：%s</p>
<p>配置：%s/config.yaml（修改后重启插件生效）</p>
</body></html>`, expiry, dir)
	})
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
		Subject:      pkix.Name{CommonName: "iotapanel HTTPS"},
		NotBefore:    time.Now().Add(-time.Hour),
		NotAfter:     time.Now().Add(825 * 24 * time.Hour),
		KeyUsage:     x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth},
	}
	if domain != "" {
		for _, d := range strings.Split(domain, ",") {
			tmpl.DNSNames = append(tmpl.DNSNames, strings.TrimSpace(d))
		}
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

func envOr(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

// 显式引用 net 包（证书生成用），避免误删。
