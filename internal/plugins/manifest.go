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

// Package plugins 实现插件生命周期管理：
// 冷启动、端口认领、空闲退出、保活、安装/卸载。
package plugins

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"

	"gopkg.in/yaml.v3"
)

// manifest 缓存：避免同一目录的 manifest.yaml 被反复读盘/解析（如每次插件列表、
// 每次免登录 MCP 探测），减少重复分配。按文件 mtime 失效。
var (
	manifestCacheMu sync.Mutex
	manifestCache   = map[string]*manifestCacheEntry{}
)

type manifestCacheEntry struct {
	mf    *Manifest
	mtime int64
}

// Manifest 是插件元信息文件 manifest.yaml 的结构。
type Manifest struct {
	Name        string   `yaml:"name"`
	Title       string   `yaml:"title"`
	Version     string   `yaml:"version"`
	Author      string   `yaml:"author"`
	Description string   `yaml:"description"`
	Language    string   `yaml:"language"`
	Bind        string   `yaml:"bind"`    // 默认 127.0.0.1
	Command     string   `yaml:"command"` // 相对插件目录的可执行入口，如 bin/file-manager
	Args        []string `yaml:"args"`
	Keepalive   bool     `yaml:"keepalive"` // 安装时默认保活
	Auth        string   `yaml:"auth"`      // "" = 需面板登录；"none" = 免面板登录（插件自鉴权，如 MCP /mcp 端点）
	Menus       []Menu   `yaml:"menus"`
}

type Menu struct {
	Title   string `yaml:"title" json:"title"`
	Icon    string `yaml:"icon" json:"icon"`
	Path    string `yaml:"path" json:"path"`       // 插件侧路由，如 /
	Section string `yaml:"section" json:"section"` // 侧边栏分组，如 tools / system
}

// LoadManifest 从插件安装目录读取并校验 manifest.yaml。
// 命中有效缓存（同目录、mtime 未变）时直接返回，避免重复读盘与解析。
func LoadManifest(dir string) (*Manifest, error) {
	path := filepath.Join(dir, "manifest.yaml")
	fi, err := os.Stat(path)
	if err != nil {
		return nil, fmt.Errorf("读取 manifest.yaml 失败: %w", err)
	}
	mtime := fi.ModTime().UnixNano()

	manifestCacheMu.Lock()
	if e, ok := manifestCache[path]; ok && e.mtime == mtime {
		mf := *e.mf
		manifestCacheMu.Unlock()
		return &mf, nil
	}
	manifestCacheMu.Unlock()

	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("读取 manifest.yaml 失败: %w", err)
	}
	var mf Manifest
	if err := yaml.Unmarshal(data, &mf); err != nil {
		return nil, fmt.Errorf("解析 manifest.yaml 失败: %w", err)
	}
	if mf.Name == "" {
		return nil, fmt.Errorf("manifest.yaml 缺少 name")
	}
	if mf.Command == "" {
		return nil, fmt.Errorf("manifest.yaml 缺少 command")
	}
	if mf.Bind == "" {
		mf.Bind = "127.0.0.1"
	}
	if mf.Title == "" {
		mf.Title = mf.Name
	}
	manifestCacheMu.Lock()
	manifestCache[path] = &manifestCacheEntry{mf: &mf, mtime: mtime}
	manifestCacheMu.Unlock()
	return &mf, nil
}
