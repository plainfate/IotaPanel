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

package api

// 商城扩展：远程 URL 安装插件（规格书流程：下载 → 哈希校验 → 解压 → 注册）。

import (
	"archive/tar"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"iotapanel/internal/db"
	"iotapanel/internal/plugins"
)

// 远程插件包大小上限（64MB，防恶意大包）
const maxRemotePluginSize = 64 << 20

// handleStoreInstallURL 从 URL 下载插件包（.tar.gz，内含 <插件名>/manifest.yaml），
// 可选 SHA256 校验，解压后注册到面板（无需重启）。
// 请求体: {"url": "https://.../my-plugin.tar.gz", "sha256": "可选校验值"}
func (s *Server) handleStoreInstallURL(w http.ResponseWriter, r *http.Request) {
	var req struct {
		URL    string `json:"url"`
		SHA256 string `json:"sha256"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.URL == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "缺少插件包下载地址"})
		return
	}
	if !strings.HasPrefix(req.URL, "http://") && !strings.HasPrefix(req.URL, "https://") {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "仅支持 http/https 下载地址"})
		return
	}

	// 1. 下载
	client := &http.Client{Timeout: 60 * time.Second}
	resp, err := client.Get(req.URL)
	if err != nil {
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": "下载失败: " + err.Error()})
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": fmt.Sprintf("下载失败: HTTP %d", resp.StatusCode)})
		return
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, maxRemotePluginSize))
	if err != nil {
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": "读取下载内容失败: " + err.Error()})
		return
	}

	// 2. SHA256 校验（可选但推荐）
	if req.SHA256 != "" {
		sum := sha256.Sum256(data)
		if hex.EncodeToString(sum[:]) != strings.ToLower(req.SHA256) {
			writeJSON(w, http.StatusBadRequest, map[string]string{"error": "SHA256 校验失败，包可能被篡改或下载不完整"})
			return
		}
	}

	// 3. 解压（gzip + tar，要求顶层一个包含 manifest.yaml 的目录）
	name, files, err := unpackPluginPackage(data)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "插件包解析失败: " + err.Error()})
		return
	}

	// 4. 复制到插件目录
	dest := filepath.Join(s.cfg.Home, "plugins", name)
	if err := os.RemoveAll(dest); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	for rel, content := range files {
		target := filepath.Join(dest, rel)
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
			return
		}
		mode := os.FileMode(0o644)
		if strings.HasPrefix(filepath.ToSlash(rel), "bin/") {
			mode = 0o755
		}
		if err := os.WriteFile(target, content, mode); err != nil {
			writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
			return
		}
	}

	// 5. 登记（保持已有 keepalive 设置）
	mf, err := plugins.LoadManifest(dest)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "manifest 解析失败: " + err.Error()})
		return
	}
	keepalive := false
	if _, installed, _ := s.db.GetPlugin(name); installed {
		keepalive = s.db.IsKeepalive(name) // 升级时保留保活设置
	}
	if err := s.db.UpsertPlugin(db.PluginRecord{
		Name: mf.Name, Title: mf.Title, Version: mf.Version,
		Author: mf.Author, Description: mf.Description,
		InstalledAt: db.Now(), Source: "remote",
	}); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	_ = s.db.SetKeepalive(name, keepalive || mf.Keepalive)
	s.log.Info("plugin installed from URL", "plugin", name, "version", mf.Version, "url", req.URL)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "plugin": name, "version": mf.Version})
}

// unpackPluginPackage 解析 gzip+tar 插件包：
// 要求包内恰好有一个顶层目录，内含 manifest.yaml。
// 返回插件名与文件映射（相对路径 -> 内容）。
func unpackPluginPackage(data []byte) (string, map[string][]byte, error) {
	gr, err := gzip.NewReader(strings.NewReader(string(data)))
	if err != nil {
		return "", nil, fmt.Errorf("不是有效的 gzip 压缩包: %w", err)
	}
	defer gr.Close()
	tr := tar.NewReader(gr)

	type entry struct {
		content []byte
		mode    int64
	}
	files := map[string]entry{}
	topDir := ""
	total := 0 // 解压后总字节数（防 gzip 炸弹：单文件 64MB 封顶 + 总量 256MB 封顶）
	for {
		hdr, err := tr.Next()
		if err == io.EOF {
			break
		}
		if err != nil {
			return "", nil, err
		}
		// 规整路径，防目录穿越
		clean := filepath.ToSlash(filepath.Clean(hdr.Name))
		if strings.HasPrefix(clean, "..") || strings.HasPrefix(clean, "/") {
			return "", nil, fmt.Errorf("非法路径: %s", hdr.Name)
		}
		parts := strings.SplitN(clean, "/", 2)
		if len(parts) < 2 {
			continue // 顶层文件忽略
		}
		if topDir == "" {
			topDir = parts[0]
		} else if parts[0] != topDir {
			return "", nil, fmt.Errorf("包内存在多个顶层目录")
		}
		if hdr.Typeflag == tar.TypeReg {
			content, err := io.ReadAll(io.LimitReader(tr, maxRemotePluginSize))
			if err != nil {
				return "", nil, err
			}
			total += len(content)
			if total > maxRemotePluginSize*4 {
				return "", nil, fmt.Errorf("插件包解压后总大小超过上限（%d MB）", maxRemotePluginSize*4>>20)
			}
			files[parts[1]] = entry{content: content, mode: hdr.Mode}
		}
	}
	if topDir == "" {
		return "", nil, fmt.Errorf("包内未找到插件目录")
	}
	if _, ok := files["manifest.yaml"]; !ok {
		return "", nil, fmt.Errorf("插件目录缺少 manifest.yaml")
	}
	out := map[string][]byte{}
	for rel, e := range files {
		out[rel] = e.content
	}
	return topDir, out, nil
}
