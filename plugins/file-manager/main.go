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

// 文件管理插件。
//
// 插件是独立于面板核心的同级进程：由核心冷启动（注入 PLUGIN_PORT 等环境变量），
// 崩溃或被杀都不影响核心。本示例用 Go 标准库实现，其他语言亦可。
//
// 功能：目录浏览（含权限/属主）、文本文件在线查看与编辑、上传、下载、
//
//	新建目录、重命名、删除；根目录可用 FM_ROOT 限制（默认 /）。
//
// 注意：前端页面里的 AJAX 一律使用【相对路径】api/...，
//
//	经面板网关转发后即为 /p/<插件名>/api/...（绝对路径会打到面板 404）。
package main

import (
	"embed"
	"encoding/json"
	"errors"
	"fmt"
	"html"
	"io"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"
)

//go:embed web
var webFS embed.FS

// root 为文件浏览根目录，可用环境变量 FM_ROOT 覆盖（默认 /）。
var root string

// 在线编辑的大小上限（防止误开大文件）
const maxEditSize = 2 << 20 // 2MB

func main() {
	root = envOr("FM_ROOT", "/")
	port := os.Getenv("PLUGIN_PORT")
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	if port == "" {
		port = "19001" // 手动运行时使用默认端口
	}

	mux := http.NewServeMux()
	mux.HandleFunc("GET /", handleIndex)                // 前端页面
	mux.HandleFunc("GET /api/list", handleList)         // 目录列表
	mux.HandleFunc("GET /api/read", handleRead)         // 读取文本文件（预览/编辑）
	mux.HandleFunc("POST /api/write", handleWrite)      // 保存文本文件
	mux.HandleFunc("GET /api/download", handleDownload) // 下载文件
	mux.HandleFunc("POST /api/upload", handleUpload)    // 上传文件
	mux.HandleFunc("POST /api/mkdir", handleMkdir)      // 新建目录
	mux.HandleFunc("POST /api/rename", handleRename)    // 重命名
	mux.HandleFunc("POST /api/delete", handleDelete)    // 删除文件/目录
	mux.HandleFunc("GET /api/disks", handleDisks)       // 挂载磁盘列表（外接硬盘等）

	addr := bind + ":" + port
	server := &http.Server{Addr: addr, Handler: mux, ReadHeaderTimeout: 10 * time.Second}

	// 优雅退出：收到 SIGTERM（面板停止/空闲退出）时关闭 HTTP 服务
	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		log.Printf("[file-manager] 收到退出信号，正在关闭")
		server.Close()
	}()

	log.Printf("[file-manager] listening on %s, root=%s", addr, root)
	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}

// ---------- 前端 ----------

// handleIndex 返回内嵌的插件页面（纯 HTML/CSS/JS）
func handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	data, err := webFS.ReadFile("web/index.html")
	if err != nil {
		http.Error(w, "页面资源缺失", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Write(data)
}

// ---------- 路径安全 ----------

// resolve 把请求中的相对路径规整为根目录内的绝对路径，防止越权访问。
// 传入的 p 为根目录内的相对路径（如 /tmp、/var/log）。
func resolve(p string) (string, error) {
	if p == "" {
		p = "/"
	}
	abs, err := filepath.Abs(filepath.Join(root, p))
	if err != nil {
		return "", err
	}
	rootAbs, _ := filepath.Abs(root)
	// 用 filepath.Rel 判断是否越界（正确处理根目录为 "/" 的情况）
	rel, err := filepath.Rel(rootAbs, abs)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
		return "", errors.New("路径越界")
	}
	return abs, nil
}

// ---------- API ----------

type entry struct {
	Name  string `json:"name"`
	Dir   bool   `json:"dir"`
	Size  int64  `json:"size"`
	Mode  string `json:"mode"`  // 权限位，如 -rw-r--r--
	Owner string `json:"owner"` // 属主:属组，如 0:0
	Mtime string `json:"mtime"`
}

// GET /api/list?path=... 返回目录下的条目列表
func handleList(w http.ResponseWriter, r *http.Request) {
	relPath := r.URL.Query().Get("path")
	dir, err := resolve(relPath)
	if err != nil {
		writeErr(w, err)
		return
	}
	items, err := os.ReadDir(dir)
	if err != nil {
		writeErr(w, err)
		return
	}
	entries := make([]entry, 0, len(items))
	for _, it := range items {
		info, err := it.Info()
		if err != nil {
			continue
		}
		owner := "-"
		if st, ok := info.Sys().(*syscall.Stat_t); ok {
			owner = fmt.Sprintf("%d:%d", st.Uid, st.Gid)
		}
		entries = append(entries, entry{
			Name:  it.Name(),
			Dir:   it.IsDir(),
			Size:  info.Size(),
			Mode:  info.Mode().String(),
			Owner: owner,
			Mtime: info.ModTime().Format("2006-01-02 15:04"),
		})
	}
	// 目录在前，按名称排序
	sort.Slice(entries, func(i, j int) bool {
		if entries[i].Dir != entries[j].Dir {
			return entries[i].Dir
		}
		return entries[i].Name < entries[j].Name
	})
	writeJSON(w, map[string]any{"path": relPath, "root": root, "entries": entries})
}

// GET /api/read?path=... 读取文本文件内容（限制 2MB，二进制返回错误）
func handleRead(w http.ResponseWriter, r *http.Request) {
	p, err := resolve(r.URL.Query().Get("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	info, err := os.Stat(p)
	if err != nil {
		writeErr(w, err)
		return
	}
	if info.IsDir() {
		writeErr(w, errors.New("目录不支持在线查看"))
		return
	}
	if info.Size() > maxEditSize {
		writeErr(w, fmt.Errorf("文件过大（超过 %dMB），请使用下载", maxEditSize>>20))
		return
	}
	data, err := os.ReadFile(p)
	if err != nil {
		writeErr(w, err)
		return
	}
	// 简单二进制检测：前 8KB 出现 NUL 字节视为二进制
	head := data
	if len(head) > 8192 {
		head = head[:8192]
	}
	if strings.ContainsRune(string(head), 0) {
		writeErr(w, errors.New("二进制文件不支持在线编辑，请使用下载"))
		return
	}
	writeJSON(w, map[string]any{"content": string(data), "size": info.Size(), "encoding": "utf-8"})
}

// POST /api/write  {"path": "...", "content": "..."} 保存文本文件（保留原权限）
func handleWrite(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path    string `json:"path"`
		Content string `json:"content"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, err)
		return
	}
	if len(req.Content) > maxEditSize {
		writeErr(w, fmt.Errorf("内容过大（超过 %dMB）", maxEditSize>>20))
		return
	}
	p, err := resolve(req.Path)
	if err != nil {
		writeErr(w, err)
		return
	}
	mode := os.FileMode(0o644)
	if info, err := os.Stat(p); err == nil && !info.IsDir() {
		mode = info.Mode()
	}
	if err := os.WriteFile(p, []byte(req.Content), mode); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, map[string]any{"ok": true, "size": len(req.Content)})
}

// GET /api/download?path=... 下载文件
func handleDownload(w http.ResponseWriter, r *http.Request) {
	p, err := resolve(r.URL.Query().Get("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	f, err := os.Open(p)
	if err != nil {
		writeErr(w, err)
		return
	}
	defer f.Close()
	info, _ := f.Stat()
	if info.IsDir() {
		writeErr(w, errors.New("不支持下载目录"))
		return
	}
	// Content-Disposition 转义文件名，支持中文
	w.Header().Set("Content-Disposition", fmt.Sprintf("attachment; filename*=UTF-8''%s", html.EscapeString(filepath.Base(p))))
	http.ServeContent(w, r, filepath.Base(p), info.ModTime(), f)
}

// POST /api/upload  multipart 上传（字段 file=文件内容，path=目标目录）
func handleUpload(w http.ResponseWriter, r *http.Request) {
	dir, err := resolve(r.FormValue("path"))
	if err != nil {
		writeErr(w, err)
		return
	}
	if st, err := os.Stat(dir); err != nil || !st.IsDir() {
		writeErr(w, errors.New("目标目录不存在"))
		return
	}
	r.ParseMultipartForm(64 << 20) // 64MB
	file, header, err := r.FormFile("file")
	if err != nil {
		writeErr(w, err)
		return
	}
	defer file.Close()
	target := filepath.Join(dir, filepath.Base(header.Filename))
	out, err := os.Create(target)
	if err != nil {
		writeErr(w, err)
		return
	}
	defer out.Close()
	if _, err := io.Copy(out, file); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, map[string]any{"ok": true, "file": header.Filename})
}

// POST /api/mkdir  {"path": "/foo/bar"} 递归新建目录
func handleMkdir(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, err)
		return
	}
	p, err := resolve(req.Path)
	if err != nil {
		writeErr(w, err)
		return
	}
	if err := os.MkdirAll(p, 0o755); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, map[string]any{"ok": true})
}

// POST /api/rename  {"path": "...", "new_name": "..."} 重命名（同一目录内）
func handleRename(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path    string `json:"path"`
		NewName string `json:"new_name"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, err)
		return
	}
	if req.NewName == "" || strings.ContainsAny(req.NewName, "/\\") ||
		req.NewName == "." || req.NewName == ".." {
		writeErr(w, errors.New("非法的新名称"))
		return
	}
	p, err := resolve(req.Path)
	if err != nil {
		writeErr(w, err)
		return
	}
	if p == root {
		writeErr(w, errors.New("不能重命名根目录"))
		return
	}
	if err := os.Rename(p, filepath.Join(filepath.Dir(p), req.NewName)); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, map[string]any{"ok": true})
}

// POST /api/delete  {"path": "..."} 删除文件或目录
func handleDelete(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Path string `json:"path"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeErr(w, err)
		return
	}
	p, err := resolve(req.Path)
	if err != nil {
		writeErr(w, err)
		return
	}
	if p == root {
		writeErr(w, errors.New("不能删除根目录"))
		return
	}
	if err := os.RemoveAll(p); err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, map[string]any{"ok": true})
}

// ---------- 挂载磁盘 ----------

type diskInfo struct {
	Mountpoint string  `json:"mountpoint"`
	Device     string  `json:"device"`
	FSType     string  `json:"fstype"`
	Total      uint64  `json:"total"`
	Used       uint64  `json:"used"`
	Free       uint64  `json:"free"`
	Percent    float64 `json:"percent"`
}

// handleDisks 列出真实磁盘/挂载点及使用率（读取 /proc/mounts + statfs），
// 外接 U 盘/移动硬盘挂载后即可在此显示并一键进入。
func handleDisks(w http.ResponseWriter, r *http.Request) {
	data, err := os.ReadFile("/proc/mounts")
	if err != nil {
		writeErr(w, err)
		return
	}
	seen := map[string]bool{}
	out := []diskInfo{}
	for _, line := range strings.Split(string(data), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 3 {
			continue
		}
		dev, mp, fstype := fields[0], fields[1], fields[2]
		// 只统计真实设备（/dev/*）与绑定/overlay 挂载，跳过 proc/sysfs/tmpfs 等伪文件系统
		if !strings.HasPrefix(dev, "/dev/") && !strings.HasPrefix(dev, "/") {
			continue
		}
		if strings.HasPrefix(fstype, "fuse.") || fstype == "squashfs" {
			continue
		}
		if seen[mp] {
			continue
		}
		seen[mp] = true
		var st syscall.Statfs_t
		if err := syscall.Statfs(mp, &st); err != nil {
			continue
		}
		total := st.Blocks * uint64(st.Bsize)
		free := st.Bavail * uint64(st.Bsize)
		used := total - free
		percent := 0.0
		if total > 0 {
			percent = float64(used) / float64(total) * 100
		}
		out = append(out, diskInfo{Mountpoint: mp, Device: dev, FSType: fstype, Total: total, Used: used, Free: free, Percent: percent})
	}
	writeJSON(w, map[string]any{"disks": out})
}

// ---------- 工具 ----------

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, err error) {
	w.WriteHeader(http.StatusBadRequest)
	writeJSON(w, map[string]string{"error": err.Error()})
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
