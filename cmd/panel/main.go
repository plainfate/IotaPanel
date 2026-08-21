// MicroPanel 面板核心入口。
//
// 极简微内核：仅负责用户认证、反向代理网关、插件进程管理。
// 常驻内存小，插件按需冷启动、空闲自动退出。
package main

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"runtime/debug"
	"strconv"
	"syscall"
	"time"

	"micropanel/internal/api"
	"micropanel/internal/config"
	"micropanel/internal/db"
	"micropanel/internal/plugins"
)

// syncPluginsFromDir 扫描 PANEL_HOME/plugins/ 下的插件目录，
// 把尚未登记到数据库的插件自动入库（拷贝即安装，无需手动配置）。
func syncPluginsFromDir(home string, database *db.DB, logger *slog.Logger) {
	dir := filepath.Join(home, "plugins")
	entries, err := os.ReadDir(dir)
	if err != nil {
		return // 目录不存在时忽略（首次安装前）
	}
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		name := e.Name()
		if _, installed, _ := database.GetPlugin(name); installed {
			continue // 已登记过，跳过
		}
		mf, err := plugins.LoadManifest(filepath.Join(dir, name))
		if err != nil {
			logger.Warn("跳过无法识别的插件目录（缺少有效 manifest.yaml）", "plugin", name)
			continue
		}
		if err := database.UpsertPlugin(db.PluginRecord{
			Name: mf.Name, Title: mf.Title, Version: mf.Version,
			Author: mf.Author, Description: mf.Description,
			InstalledAt: db.Now(), Source: "local",
		}); err != nil {
			logger.Warn("登记插件失败", "plugin", name, "err", err)
			continue
		}
		if mf.Keepalive {
			_ = database.SetKeepalive(name, true)
		}
		logger.Info("自动登记插件（拷贝即安装）", "plugin", name, "version", mf.Version)
	}
}

func main() {
	// 命令行子命令（panel start/stop/restart/status/log/version/help）
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "-version", "--version", "-v", "version":
			fmt.Printf("MicroPanel %s\n", config.Version)
			os.Exit(0)
		case "start", "stop", "restart", "uninstall", "status", "log", "help", "-h", "--help":
			runCLI(os.Args[1:])
			os.Exit(0)
		}
		// 其余未知参数（如 serve）继续作为服务启动
	}

	// 极简资源策略：给 Go 堆设一个硬上限（可被 GOMEMLIMIT 环境变量覆盖）。
	// 注意：不要改 GOGC——保持默认 100，突发流量后内存才能及时回收，
	// 实测空闲常驻约 11MB（含 27MB 二进制中常被触达的代码页）。
	if os.Getenv("GOMEMLIMIT") == "" {
		debug.SetMemoryLimit(48 << 20) // 堆上限 48MB，防止突发请求撑大常驻内存
	}

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))

	cfg, err := config.Load()
	if err != nil {
		logger.Error("加载配置失败", "err", err)
		os.Exit(1)
	}

	// 核心日志同时写入 logs/panel.log（规格书目录结构）
	logDir := filepath.Join(cfg.Home, "logs")
	os.MkdirAll(logDir, 0o755)
	if f, err := os.OpenFile(filepath.Join(logDir, "panel.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644); err == nil {
		logger = slog.New(slog.NewTextHandler(io.MultiWriter(os.Stderr, f), &slog.HandlerOptions{Level: slog.LevelInfo}))
	}

	// 记录安装目录标记：非 systemd 环境下 `panel start` 用它恢复 PANEL_HOME
	_ = os.WriteFile("/tmp/micropanel-home", []byte(cfg.Home+"\n"), 0o644)

	database, err := db.Open(cfg.Home)
	if err != nil {
		logger.Error("初始化数据库失败", "err", err, "home", cfg.Home)
		os.Exit(1)
	}
	defer database.Close()

	// 扫描 plugins/ 目录：手动放入的插件目录自动登记（拷贝即安装）
	syncPluginsFromDir(cfg.Home, database, logger)

	// 设置页持久化的空闲退出时间优先于 .env
	if v, ok := database.GetSetting("idle_timeout_minutes"); ok {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			cfg.IdleTimeout = time.Duration(n) * time.Minute
		}
	}

	mgr := plugins.NewManager(cfg.Home, cfg.IdleTimeout, cfg.PortLo, cfg.PortHi, database, logger)
	mgr.Load() // 扫描 port-map.json，认领仍存活的插件进程

	srv := api.NewServer(cfg, database, mgr, logger)
	httpSrv := &http.Server{
		Addr:              cfg.ListenAddr,
		Handler:           srv.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}

	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		logger.Info("收到退出信号，开始清理")
		mgr.Shutdown() // 仅停止非保活插件；保活插件进程保留，重启后复用端口
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = httpSrv.Shutdown(ctx)
		logger.Info("面板核心已退出")
		os.Exit(0)
	}()

	logger.Info("MicroPanel 启动",
		"version", config.Version,
		"addr", cfg.ListenAddr,
		"home", cfg.Home,
		"idle_timeout", cfg.IdleTimeout.String(),
		"port_pool", strconv.Itoa(cfg.PortLo)+"-"+strconv.Itoa(cfg.PortHi),
	)
	if err := httpSrv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		logger.Error("HTTP 服务异常退出", "err", err)
		os.Exit(1)
	}
}
