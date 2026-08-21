// Package plugins 实现插件生命周期管理：
// 冷启动、端口认领、空闲退出、保活、安装/卸载。
package plugins

import (
	"fmt"
	"os"
	"path/filepath"

	"gopkg.in/yaml.v3"
)

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
	Menus       []Menu   `yaml:"menus"`
}

type Menu struct {
	Title   string `yaml:"title" json:"title"`
	Icon    string `yaml:"icon" json:"icon"`
	Path    string `yaml:"path" json:"path"`       // 插件侧路由，如 /
	Section string `yaml:"section" json:"section"` // 侧边栏分组，如 tools / system
}

// LoadManifest 从插件安装目录读取并校验 manifest.yaml。
func LoadManifest(dir string) (*Manifest, error) {
	data, err := os.ReadFile(filepath.Join(dir, "manifest.yaml"))
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
	return &mf, nil
}
