// 网页终端插件（仅 Linux）。
//
// 轻量设计：xterm.js 前端 + WebSocket + PTY（伪终端），
// 一个连接一个 goroutine，无轮询。由面板核心按需冷启动。
//
// 安全：默认只监听 127.0.0.1，必须通过面板网关（登录后）访问。
package main

import (
	"embed"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"strings"
	"syscall"

	"github.com/creack/pty"
	"github.com/gorilla/websocket"
)

//go:embed web
var webFS embed.FS

var upgrader = websocket.Upgrader{
	ReadBufferSize:  4096,
	WriteBufferSize: 4096,
	// 终端页面经面板网关 iframe 嵌入，Origin 与面板同源，直接放行
	CheckOrigin: func(r *http.Request) bool { return true },
}

func main() {
	port := os.Getenv("PLUGIN_PORT")
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	if port == "" {
		port = "19004" // 手动运行时使用默认端口
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /", handleStatic)
	mux.HandleFunc("GET /ws", handleWS)

	addr := bind + ":" + port
	server := &http.Server{Addr: addr, Handler: mux}
	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		server.Close()
	}()
	log.Printf("[terminal] listening on %s", addr)
	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}

// handleStatic 服务内嵌静态资源：/ 返回页面，其余按路径返回 lib/ 等资源。
func handleStatic(w http.ResponseWriter, r *http.Request) {
	p := strings.TrimPrefix(r.URL.Path, "/")
	if p == "" {
		p = "index.html"
	}
	data, err := webFS.ReadFile("web/" + p)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", mimeType(p))
	w.Write(data)
}

// mimeType 按扩展名返回 Content-Type。
func mimeType(name string) string {
	switch {
	case strings.HasSuffix(name, ".js"):
		return "application/javascript; charset=utf-8"
	case strings.HasSuffix(name, ".css"):
		return "text/css; charset=utf-8"
	case strings.HasSuffix(name, ".html"):
		return "text/html; charset=utf-8"
	case strings.HasSuffix(name, ".woff2"):
		return "font/woff2"
	case strings.HasSuffix(name, ".png"):
		return "image/png"
	case strings.HasSuffix(name, ".svg"):
		return "image/svg+xml"
	default:
		return "application/octet-stream"
	}
}

// handleWS 建立 WebSocket，桥接浏览器与 PTY。
func handleWS(w http.ResponseWriter, r *http.Request) {
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()

	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/bash"
	}
	cmd := exec.Command(shell)
	cmd.Env = append(os.Environ(), "TERM=xterm-256color")
	f, err := pty.Start(cmd)
	if err != nil {
		_ = conn.WriteMessage(websocket.TextMessage, []byte("启动 shell 失败: "+err.Error()))
		return
	}
	defer func() {
		_ = f.Close()
		_ = cmd.Wait()
	}()

	// PTY -> WebSocket（输出）
	go func() {
		buf := make([]byte, 4096)
		for {
			n, err := f.Read(buf)
			if n > 0 {
				if werr := conn.WriteMessage(websocket.BinaryMessage, buf[:n]); werr != nil {
					return
				}
			}
			if err != nil {
				return
			}
		}
	}()

	// WebSocket -> PTY（输入 + 尺寸调整）
	// 协议：普通文本/二进制直接写入；JSON {"type":"resize","cols":..,"rows":..} 调整尺寸
	for {
		_, data, err := conn.ReadMessage()
		if err != nil {
			return
		}
		var msg struct {
			Type string `json:"type"`
			Cols uint16 `json:"cols"`
			Rows uint16 `json:"rows"`
		}
		if json.Unmarshal(data, &msg) == nil && msg.Type == "resize" && msg.Cols > 0 && msg.Rows > 0 {
			_ = pty.Setsize(f, &pty.Winsize{Rows: msg.Rows, Cols: msg.Cols})
			continue
		}
		_, _ = f.Write(data)
	}
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
