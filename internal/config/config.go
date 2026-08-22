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

// Package config 负责加载面板核心的运行时配置。
// 配置来源：环境变量 -> PANEL_HOME/etc/.env（环境变量优先）。
package config

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Version 面板核心版本号（可用 ldflags 覆盖）。
const Version = "0.3.3"

type Config struct {
	Home        string        // PANEL_HOME 安装目录（用户自定义，不强制 /opt）
	ListenAddr  string        // 面板 HTTP 监听地址
	JWTSecret   string        // 会话签名密钥
	IdleTimeout time.Duration // 插件空闲退出时间
	PortLo      int           // 插件端口池下限
	PortHi      int           // 插件端口池上限
	TrustProxy  bool          // PANEL_TRUST_PROXY：部署在受信反代之后才信任 X-Forwarded-* 头
}

// Load 读取环境变量与 .env 文件，构造配置。
// PANEL_HOME 的确定顺序：
//  1. 环境变量 PANEL_HOME（systemd EnvironmentFile / 手动 export 优先）
//  2. .env 文件中的 PANEL_HOME
//  3. 按可执行文件位置推导（<安装目录>/bin/panel -> 安装目录）
//  4. 兜底 /data/panel
//
// 若 JWT_SECRET 缺失则自动生成并持久化到 .env。
func Load() (*Config, error) {
	home := os.Getenv("PANEL_HOME")
	if home == "" {
		home = deriveHomeFromExecutable()
	}
	loadEnvFile(filepath.Join(home, "etc", ".env"))
	if v := os.Getenv("PANEL_HOME"); v != "" { // .env 中可能写有 PANEL_HOME
		home = v
	}
	if home == "" {
		home = "/data/panel"
	}

	cfg := &Config{
		Home: home,
		// 默认 ":8787"：在全部网卡上监听（IPv4 + IPv6 双栈）。
		// 仅本机调试可设 127.0.0.1:8787；仅 IPv4 全接口可设 0.0.0.0:8787。
		ListenAddr:  envOr("LISTEN_ADDR", ":8787"),
		JWTSecret:   os.Getenv("JWT_SECRET"),
		IdleTimeout: 5 * time.Minute,
		PortLo:      19000,
		PortHi:      19999,
	}
	if v := os.Getenv("IDLE_TIMEOUT"); v != "" {
		if d, err := time.ParseDuration(v); err == nil && d > 0 {
			cfg.IdleTimeout = d
		}
	}
	if v := os.Getenv("PORT_START"); v != "" {
		fmt.Sscanf(v, "%d", &cfg.PortLo)
	}
	if v := os.Getenv("PORT_END"); v != "" {
		fmt.Sscanf(v, "%d", &cfg.PortHi)
	}
	// 仅当面板部署在受信反向代理之后才信任 X-Forwarded-* 头（CSRF 校验 / 协议推断用）。
	// 直连模式（默认）忽略这些头，防止客户端伪造头绕过 Origin 校验。
	if v := os.Getenv("PANEL_TRUST_PROXY"); v == "1" || strings.EqualFold(v, "true") {
		cfg.TrustProxy = true
	}

	if cfg.JWTSecret == "" {
		secret, err := generateSecret()
		if err != nil {
			return nil, err
		}
		cfg.JWTSecret = secret
		if err := saveEnvVar(filepath.Join(home, "etc", ".env"), "JWT_SECRET", secret); err != nil {
			return nil, fmt.Errorf("写入 JWT_SECRET 失败: %w", err)
		}
	}
	return cfg, nil
}

// SetEnvVar 写入/更新 .env 中的某个键值（设置页改端口等场景用）。
func SetEnvVar(home, key, value string) error {
	return saveEnvVar(filepath.Join(home, "etc", ".env"), key, value)
}

// 解析简单的 KEY=VALUE 格式 .env，不覆盖已有环境变量。
func loadEnvFile(path string) {
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	sc := bufio.NewScanner(strings.NewReader(string(data)))
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		k, v, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		k = strings.TrimSpace(k)
		v = strings.Trim(strings.TrimSpace(v), `"'`)
		if k != "" && os.Getenv(k) == "" {
			os.Setenv(k, v)
		}
	}
}

// 向 .env 写入或更新一个键值。
func saveEnvVar(path, key, value string) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	lines := []string{}
	if data, err := os.ReadFile(path); err == nil {
		found := false
		for _, ln := range strings.Split(string(data), "\n") {
			trimmed := strings.TrimSpace(ln)
			if strings.HasPrefix(trimmed, key+"=") {
				lines = append(lines, key+"="+value)
				found = true
				continue
			}
			lines = append(lines, ln)
		}
		if !found {
			lines = append(lines, key+"="+value)
		}
	} else {
		lines = append(lines, key+"="+value)
	}
	return os.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0o600)
}

func generateSecret() (string, error) {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return hex.EncodeToString(b), nil
}

// deriveHomeFromExecutable 根据二进制位置推导安装目录：
// 标准布局为 <安装目录>/bin/panel，故取其父目录的父目录。
func deriveHomeFromExecutable() string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	dir := filepath.Dir(exe) // .../bin
	if filepath.Base(dir) != "bin" {
		return ""
	}
	parent := filepath.Dir(dir)
	if parent == "/" || parent == "." {
		return ""
	}
	return parent
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
