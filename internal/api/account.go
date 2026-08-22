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

// 账户与安全：
//   - 账户信息 / 修改密码（改密后强制其他会话下线）
//   - 登录会话管理（列表、单条强制下线、全部下线）
//   - 登录安全策略（失败次数上限 + 锁定分钟数）

import (
	"encoding/json"
	"net"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"

	"iotapanel/internal/auth"
)

// loginGuard 登录失败锁定（内存态；重启后清零，符合极简内核设计）。
type loginGuard struct {
	mu    sync.Mutex
	fails map[string]int       // 用户名 -> 连续失败次数
	until map[string]time.Time // 用户名 -> 锁定截止时间
}

// remaining 返回剩余锁定时间；未锁定时为 0。
func (g *loginGuard) remaining(username string) time.Duration {
	g.mu.Lock()
	defer g.mu.Unlock()
	until, ok := g.until[username]
	if !ok || time.Now().After(until) {
		return 0
	}
	return time.Until(until)
}

// recordFail 记录一次失败；达到上限则锁定，返回是否触发锁定。
func (g *loginGuard) recordFail(username string, limit, lockMinutes int) (locked bool, remaining time.Duration) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.fails[username]++
	if g.fails[username] >= limit {
		until := time.Now().Add(time.Duration(lockMinutes) * time.Minute)
		g.until[username] = until
		return true, time.Until(until)
	}
	return false, 0
}

// reset 登录成功后清零失败记录与锁定。
func (g *loginGuard) reset(username string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	delete(g.fails, username)
	delete(g.until, username)
}

// securityPolicy 读取登录安全策略（默认 5 次 / 15 分钟，设置页可调）。
func (s *Server) securityPolicy() (failLimit, lockMinutes int) {
	failLimit, lockMinutes = 5, 15
	if v, ok := s.db.GetSetting("login_fail_limit"); ok {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			failLimit = n
		}
	}
	if v, ok := s.db.GetSetting("login_fail_lock_minutes"); ok {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			lockMinutes = n
		}
	}
	return failLimit, lockMinutes
}

// clientIP 提取客户端 IP（去掉端口）。
func clientIP(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

// truncate 截断过长的 User-Agent 等字段。
func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}

// ---------- 账户信息 ----------

// handleAccount 返回当前账户信息（用户名、创建时间、最近登录）。
func (s *Server) handleAccount(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	u, err := s.db.GetUserByName(sess.Username)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	failLimit, lockMin := s.securityPolicy()
	writeJSON(w, http.StatusOK, map[string]any{
		"username":      u.Username,
		"created_at":    u.CreatedAt,
		"last_login_at": u.LastLoginAt,
		"security": map[string]any{
			"fail_limit":          failLimit,
			"lock_minutes":        lockMin,
			"session_max_age_h":   24,
			"current_session_jti": sess.JTI,
		},
	})
}

// handleUsernameChange 修改用户名：校验合法性/唯一性 -> 更新（会话同步）。
func (s *Server) handleUsernameChange(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	var req struct {
		NewUsername string `json:"new_username"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	name := strings.TrimSpace(req.NewUsername)
	if len(name) < 3 || len(name) > 32 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "用户名需为 3-32 个字符"})
		return
	}
	if strings.ContainsAny(name, " /\t\n\\\"'") {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "用户名不能包含空格或特殊字符"})
		return
	}
	if name == sess.Username {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "新用户名与当前相同"})
		return
	}
	if _, err := s.db.GetUserByName(name); err == nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "用户名已被占用"})
		return
	}
	if err := s.db.UpdateUsername(sess.Username, name); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	s.log.Info("username changed", "from", sess.Username, "to", name)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "username": name})
}

// handleAccountPassword 修改密码：校验旧密码 -> 更新 -> 强制其他会话下线。
func (s *Server) handleAccountPassword(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	var req struct {
		OldPassword string `json:"old_password"`
		NewPassword string `json:"new_password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	if len(req.NewPassword) < 6 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "新密码至少 6 位"})
		return
	}
	u, err := s.db.GetUserByName(sess.Username)
	if err != nil || !auth.VerifyPassword(req.OldPassword, u.Salt, u.PasswordHash) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "原密码错误"})
		return
	}
	salt, hash, err := auth.HashPassword(req.NewPassword)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	if err := s.db.UpdatePassword(u.Username, hash, salt); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	// 改密后强制下线除当前会话外的所有会话（主流面板通行策略）
	revoked, _ := s.db.RevokeOtherSessions(u.Username, sess.JTI)
	s.log.Info("password changed, other sessions revoked", "username", u.Username, "revoked", revoked)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "revoked_sessions": revoked})
}

// ---------- 会话管理 ----------

// handleSessionsList 列出当前用户全部活跃会话，标记当前会话。
func (s *Server) handleSessionsList(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	list, err := s.db.ListSessions(sess.Username)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	type view struct {
		JTI       string `json:"jti"`
		IP        string `json:"ip"`
		UserAgent string `json:"user_agent"`
		CreatedAt string `json:"created_at"`
		ExpiresAt string `json:"expires_at"`
		Current   bool   `json:"current"`
		Revoked   bool   `json:"revoked"`
	}
	out := []view{}
	for _, s2 := range list {
		out = append(out, view{
			JTI: s2.JTI, IP: s2.IP, UserAgent: s2.UserAgent,
			CreatedAt: s2.CreatedAt, ExpiresAt: s2.ExpiresAt,
			Current: s2.JTI == sess.JTI, Revoked: s2.Revoked,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"sessions": out})
}

// handleSessionRevoke 强制下线指定会话（不允许下线当前会话）。
func (s *Server) handleSessionRevoke(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	var req struct {
		JTI string `json:"jti"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil || req.JTI == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "缺少会话标识"})
		return
	}
	if req.JTI == sess.JTI {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "不能下线当前登录会话（请使用退出登录）"})
		return
	}
	if err := s.db.RevokeSessionByJTI(req.JTI); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	s.log.Info("session revoked", "username", sess.Username, "jti", req.JTI)
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}

// handleSessionsRevokeAll 下线除当前会话外的所有会话。
func (s *Server) handleSessionsRevokeAll(w http.ResponseWriter, r *http.Request) {
	sess := sessionFrom(r)
	n, err := s.db.RevokeOtherSessions(sess.Username, sess.JTI)
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"ok": true, "revoked": n})
}

// ---------- 安全策略 ----------

// handleSecurityGet 返回当前登录安全策略。
func (s *Server) handleSecurityGet(w http.ResponseWriter, r *http.Request) {
	failLimit, lockMin := s.securityPolicy()
	writeJSON(w, http.StatusOK, map[string]any{
		"fail_limit":   failLimit,
		"lock_minutes": lockMin,
	})
}

// handleSecurityPut 保存登录安全策略。
func (s *Server) handleSecurityPut(w http.ResponseWriter, r *http.Request) {
	var req struct {
		FailLimit   int `json:"fail_limit"`
		LockMinutes int `json:"lock_minutes"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "请求格式错误"})
		return
	}
	if req.FailLimit < 1 || req.FailLimit > 100 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "失败次数上限需在 1-100 之间"})
		return
	}
	if req.LockMinutes < 1 || req.LockMinutes > 1440 {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "锁定时间需在 1-1440 分钟之间"})
		return
	}
	_ = s.db.SetSetting("login_fail_limit", strconv.Itoa(req.FailLimit))
	_ = s.db.SetSetting("login_fail_lock_minutes", strconv.Itoa(req.LockMinutes))
	writeJSON(w, http.StatusOK, map[string]any{"ok": true})
}
