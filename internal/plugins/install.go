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

package plugins

import (
	"bytes"
	"compress/gzip"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

// CatalogItem 商城条目（来自内嵌官方插件包）。
type CatalogItem struct {
	Name        string `json:"name"`
	Title       string `json:"title"`
	Version     string `json:"version"`
	Author      string `json:"author"`
	Description string `json:"description"`
	Language    string `json:"language"`
	Installed   bool   `json:"installed"`
}

// ListCatalog 枚举内嵌插件目录中的 manifest.yaml。
func ListCatalog(emb fs.FS) ([]CatalogItem, error) {
	dirs, err := fs.ReadDir(emb, "plugins")
	if err != nil {
		return nil, err
	}
	items := []CatalogItem{}
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		data, err := fs.ReadFile(emb, path.Join("plugins", d.Name(), "manifest.yaml"))
		if err != nil {
			continue
		}
		var mf Manifest
		if err := yaml.Unmarshal(data, &mf); err != nil {
			continue
		}
		items = append(items, CatalogItem{
			Name: mf.Name, Title: mf.Title, Version: mf.Version,
			Author: mf.Author, Description: mf.Description, Language: mf.Language,
		})
	}
	return items, nil
}

// CatalogContains 判断内嵌插件包中是否存在该插件。
func CatalogContains(emb fs.FS, name string) bool {
	_, err := fs.Stat(emb, path.Join("plugins", name, "manifest.yaml"))
	return err == nil
}

// InstallFromEmbed 将内嵌插件包完整复制到 PANEL_HOME/plugins/<name>，
// bin/ 下所有文件赋予可执行权限。
func InstallFromEmbed(emb fs.FS, home, name string) error {
	base := path.Join("plugins", name)
	if _, err := fs.Stat(emb, path.Join(base, "manifest.yaml")); err != nil {
		return fmt.Errorf("插件包不存在: %s", name)
	}
	dest := filepath.Join(home, "plugins", name)
	if err := os.RemoveAll(dest); err != nil {
		return err
	}
	err := fs.WalkDir(emb, base, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(base, p)
		target := filepath.Join(dest, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := fs.ReadFile(emb, p)
		if err != nil {
			return err
		}
		mode := fs.FileMode(0o644)
		if strings.Contains(p, "/bin/") {
			mode = 0o755
			// 内嵌的插件二进制以 .gz 存储（build.sh 压缩），安装时解压
			if strings.HasSuffix(p, ".gz") {
				gr, err := gzip.NewReader(bytes.NewReader(data))
				if err != nil {
					return fmt.Errorf("解压插件失败: %w", err)
				}
				raw, err := io.ReadAll(gr)
				gr.Close()
				if err != nil {
					return err
				}
				data = raw
				target = strings.TrimSuffix(target, ".gz")
			}
		}
		if err := os.WriteFile(target, data, mode); err != nil {
			return err
		}
		return nil
	})
	if err != nil {
		return err
	}
	// 兜底：给 bin 目录下所有文件加执行权限（嵌入式 FS 模式位可能丢失）
	binDir := filepath.Join(dest, "bin")
	if entries, err := os.ReadDir(binDir); err == nil {
		for _, e := range entries {
			os.Chmod(filepath.Join(binDir, e.Name()), 0o755)
		}
	}
	return nil
}

// Uninstall 停止并删除插件目录。
func (m *Manager) Uninstall(name string) error {
	_ = m.Stop(name)
	dir := filepath.Join(m.Home, "plugins", name)
	if err := os.RemoveAll(dir); err != nil {
		return err
	}
	return nil
}
