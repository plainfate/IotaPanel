// SPDX-License-Identifier: GPL-3.0-or-later
//
// Copyright (C) 2026 plainfate <https://github.com/plainfate>
//
// MicroPanel is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// MicroPanel is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with MicroPanel.  If not, see <https://www.gnu.org/licenses/>.

// Package db 封装面板持久化存储（轻量 JSON 文件存储）。
//
// 设计取舍：最初使用 SQLite（modernc 纯 Go 驱动），但驱动使核心二进制
// 增大约 8MB、常驻内存多 2-3MB，与"极简"目标冲突。
// 现改为单一 JSON 文件（data/panel.json）：内存中维护 + 变更时原子写盘。
// 单管理员/单进程场景下足够安全，且把二进制体积与内存降到最低。
package db

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// DB 是持久化存储句柄。
type DB struct {
	mu   sync.Mutex
	path string
	data *storeData
}

// storeData 是持久化数据的整体结构。
type storeData struct {
	Users    []User            `json:"users"`
	Plugins  []PluginRecord    `json:"plugins"`
	Sessions []Session         `json:"sessions"`
	Settings map[string]string `json:"settings"`
}

// User 用户记录。
type User struct {
	ID           int64  `json:"id"`
	Username     string `json:"username"`
	PasswordHash string `json:"password_hash"`
	Salt         string `json:"salt"`
	CreatedAt    string `json:"created_at"`
	LastLoginAt  string `json:"last_login_at"`
}

// PluginRecord 插件登记记录。
type PluginRecord struct {
	Name        string `json:"name"`
	Title       string `json:"title"`
	Version     string `json:"version"`
	Author      string `json:"author"`
	Description string `json:"description"`
	Keepalive   bool   `json:"keepalive"`
	InstalledAt string `json:"installed_at"`
	Source      string `json:"source"`
}

// Session 登录会话记录（令牌只存 SHA-256 指纹，不存明文）。
type Session struct {
	ID        int64  `json:"id"`
	TokenHash string `json:"token_hash"`
	JTI       string `json:"jti"`
	Username  string `json:"username"`
	IP        string `json:"ip"`
	UserAgent string `json:"user_agent"`
	CreatedAt string `json:"created_at"`
	ExpiresAt string `json:"expires_at"`
	Revoked   bool   `json:"revoked"`
}

// Open 打开（或创建）存储文件。
func Open(home string) (*DB, error) {
	dataDir := filepath.Join(home, "data")
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return nil, err
	}
	path := filepath.Join(dataDir, "panel.json")
	d := &DB{path: path, data: &storeData{Settings: map[string]string{}}}
	if data, err := os.ReadFile(path); err == nil {
		if err := json.Unmarshal(data, d.data); err != nil {
			// 主文件存在但损坏：回退到上一份备份（与注释一致）
			if bak, berr := os.ReadFile(path + ".bak"); berr == nil {
				if uerr := json.Unmarshal(bak, d.data); uerr != nil {
					return nil, fmt.Errorf("解析 panel.json 失败: %w", err)
				}
			} else {
				return nil, fmt.Errorf("解析 panel.json 失败: %w", err)
			}
		}
	} else if data, err := os.ReadFile(path+".bak"); err == nil {
		// 主文件缺失：回退到上一份备份
		if err := json.Unmarshal(data, d.data); err != nil {
			return nil, fmt.Errorf("解析 panel.json.bak 失败: %w", err)
		}
	}
	if d.data.Settings == nil {
		d.data.Settings = map[string]string{}
	}
	// 清理上次异常退出可能残留的临时文件（正常保存会 rename 掉，不会留下）
	_ = os.Remove(path + ".tmp")
	return d, nil
}

// Close 落盘并关闭。
func (d *DB) Close() error { return d.save() }

// save 原子写盘（临时文件 + rename），写入前把旧文件保留为 .bak（可回滚）。
func (d *DB) save() error {
	data, err := json.MarshalIndent(d.data, "", "  ")
	if err != nil {
		return err
	}
	if _, err := os.Stat(d.path); err == nil {
		_ = os.Rename(d.path, d.path+".bak") // 上一份数据备份
	}
	tmp := d.path + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, d.path)
}

// Now 返回当前时间的 RFC3339 字符串（统一时间格式用）。
func Now() string { return time.Now().Format(time.RFC3339) }

// ---------- 用户 ----------

// HasAdmin 判断是否已存在管理员（用于初始化向导与登录开关）。
func (d *DB) HasAdmin() (bool, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	return len(d.data.Users) > 0, nil
}

// CreateUser 写入一个新用户（username 唯一）。
func (d *DB) CreateUser(u User) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, existing := range d.data.Users {
		if existing.Username == u.Username {
			return fmt.Errorf("用户名已存在")
		}
	}
	u.ID = int64(len(d.data.Users) + 1)
	if u.CreatedAt == "" {
		u.CreatedAt = Now()
	}
	d.data.Users = append(d.data.Users, u)
	return d.save()
}

// GetUserByName 按用户名查询用户；不存在时返回 error。
func (d *DB) GetUserByName(name string) (User, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, u := range d.data.Users {
		if u.Username == name {
			return u, nil
		}
	}
	return User{}, fmt.Errorf("用户不存在: %s", name)
}

// UpdatePassword 更新用户口令哈希与盐。
func (d *DB) UpdatePassword(username, hash, salt string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Users {
		if d.data.Users[i].Username == username {
			d.data.Users[i].PasswordHash = hash
			d.data.Users[i].Salt = salt
			return d.save()
		}
	}
	return fmt.Errorf("用户不存在: %s", username)
}

// UpdateLastLogin 记录最近登录时间。
func (d *DB) UpdateLastLogin(username string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Users {
		if d.data.Users[i].Username == username {
			d.data.Users[i].LastLoginAt = Now()
			return d.save()
		}
	}
	return fmt.Errorf("用户不存在: %s", username)
}

// UpdateUsername 修改用户名（事务内同步会话表，保证已登录会话仍有效）。
func (d *DB) UpdateUsername(oldName, newName string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Users {
		if d.data.Users[i].Username == oldName {
			d.data.Users[i].Username = newName
		}
	}
	for i := range d.data.Sessions {
		if d.data.Sessions[i].Username == oldName {
			d.data.Sessions[i].Username = newName
		}
	}
	return d.save()
}

// ---------- 插件 ----------

// ListPlugins 返回全部已安装插件（按标题排序）。
func (d *DB) ListPlugins() ([]PluginRecord, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	out := make([]PluginRecord, len(d.data.Plugins))
	copy(out, d.data.Plugins)
	// 简单排序：按标题
	for i := 1; i < len(out); i++ {
		for j := i; j > 0 && out[j].Title < out[j-1].Title; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return out, nil
}

// GetPlugin 按名查询插件；第二个返回值为是否存在。
func (d *DB) GetPlugin(name string) (PluginRecord, bool, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, p := range d.data.Plugins {
		if p.Name == name {
			return p, true, nil
		}
	}
	return PluginRecord{}, false, nil
}

// UpsertPlugin 写入/更新插件记录（安装或升级时调用，keepalive 单独维护）。
func (d *DB) UpsertPlugin(p PluginRecord) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Plugins {
		if d.data.Plugins[i].Name == p.Name {
			d.data.Plugins[i] = p
			return d.save()
		}
	}
	if p.InstalledAt == "" {
		p.InstalledAt = Now()
	}
	d.data.Plugins = append(d.data.Plugins, p)
	return d.save()
}

// DeletePlugin 删除插件记录（卸载时调用）。
func (d *DB) DeletePlugin(name string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Plugins {
		if d.data.Plugins[i].Name == name {
			d.data.Plugins = append(d.data.Plugins[:i], d.data.Plugins[i+1:]...)
			return d.save()
		}
	}
	return nil
}

// SetKeepalive 设置插件的「后台保活」标记。
func (d *DB) SetKeepalive(name string, v bool) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Plugins {
		if d.data.Plugins[i].Name == name {
			d.data.Plugins[i].Keepalive = v
			return d.save()
		}
	}
	return nil
}

// IsInstalled 判断插件是否已安装（实现 plugins.Store 接口）。
func (d *DB) IsInstalled(name string) bool {
	_, ok, _ := d.GetPlugin(name)
	return ok
}

// IsKeepalive 判断插件是否开启了后台保活（实现 plugins.Store 接口）。
func (d *DB) IsKeepalive(name string) bool {
	p, ok, _ := d.GetPlugin(name)
	return ok && p.Keepalive
}

// ---------- 设置 ----------

// GetSetting 读取设置项；不存在时 ok=false。
func (d *DB) GetSetting(key string) (string, bool) {
	d.mu.Lock()
	defer d.mu.Unlock()
	v, ok := d.data.Settings[key]
	return v, ok
}

// SetSetting 写入/更新设置项（upsert）。
func (d *DB) SetSetting(key, value string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.data.Settings[key] = value
	return d.save()
}

// ---------- 会话 ----------

// CreateSession 记录一个登录会话。
func (d *DB) CreateSession(s Session) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	s.ID = int64(len(d.data.Sessions) + 1)
	if s.CreatedAt == "" {
		s.CreatedAt = Now()
	}
	d.data.Sessions = append(d.data.Sessions, s)
	return d.save()
}

// GetSessionByTokenHash 按令牌哈希查询会话（认证中间件用）。
func (d *DB) GetSessionByTokenHash(hash string) (Session, bool, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	for _, s := range d.data.Sessions {
		if s.TokenHash == hash {
			return s, true, nil
		}
	}
	return Session{}, false, nil
}

// ListSessions 返回某用户的全部会话（按创建时间倒序）。
func (d *DB) ListSessions(username string) ([]Session, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	out := []Session{}
	for _, s := range d.data.Sessions {
		if s.Username == username {
			out = append(out, s)
		}
	}
	// 倒序：新会话在前
	for i, j := 0, len(out)-1; i < j; i, j = i+1, j-1 {
		out[i], out[j] = out[j], out[i]
	}
	return out, nil
}

// RevokeSessionByJTI 按 jti 下线指定会话。
func (d *DB) RevokeSessionByJTI(jti string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Sessions {
		if d.data.Sessions[i].JTI == jti {
			d.data.Sessions[i].Revoked = true
			return d.save()
		}
	}
	return nil
}

// RevokeSessionByTokenHash 按令牌 SHA-256 指纹吊销会话（logout 用：
// 无需校验签名，只要 cookie 值还在就能吊销对应服务端记录）。
func (d *DB) RevokeSessionByTokenHash(hash string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Sessions {
		if d.data.Sessions[i].TokenHash == hash && !d.data.Sessions[i].Revoked {
			d.data.Sessions[i].Revoked = true
			return d.save()
		}
	}
	return nil
}

// RevokeOtherSessions 下线某用户除 keepJTI 外的所有会话（改密码后调用），返回下线数量。
func (d *DB) RevokeOtherSessions(username, keepJTI string) (int64, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	n := int64(0)
	for i := range d.data.Sessions {
		if d.data.Sessions[i].Username == username &&
			d.data.Sessions[i].JTI != keepJTI && !d.data.Sessions[i].Revoked {
			d.data.Sessions[i].Revoked = true
			n++
		}
	}
	if n > 0 {
		return n, d.save()
	}
	return 0, nil
}

// RevokeAllSessions 下线某用户全部会话。
func (d *DB) RevokeAllSessions(username string) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	for i := range d.data.Sessions {
		if d.data.Sessions[i].Username == username {
			d.data.Sessions[i].Revoked = true
		}
	}
	return d.save()
}
