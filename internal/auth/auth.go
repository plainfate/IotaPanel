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

// Package auth 实现口令哈希（PBKDF2）与会话令牌（HMAC-SHA256 签名 cookie）。
package auth

import (
	"crypto/hmac"
	"crypto/pbkdf2"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"hash"
	"strconv"
	"strings"
	"time"
)

const (
	// iterations 为 PBKDF2-SHA256 迭代次数（OWASP 2023 建议 ≥ 60 万次）。
	iterations = 600_000
	// legacyIterations 兼容旧版 10 万次迭代存储的哈希（登录成功后自动升级）。
	legacyIterations = 100_000
	keyLen           = 32
	saltLen          = 16
	CookieName       = "mp_session"
	sessionTTL       = 24 * time.Hour
)

// HashPassword 生成 salt（格式 "iterations:hex"，带迭代次数便于未来调整）与 PBKDF2 哈希。
func HashPassword(pw string) (salt, hashHex string, err error) {
	saltBytes := make([]byte, saltLen)
	if _, err = rand.Read(saltBytes); err != nil {
		return "", "", err
	}
	// Go 1.25+ 的 pbkdf2.Key：签名变为 Key(h func() Hash, password string, salt, iter, keyLen) ([]byte, error)
	dk, err := pbkdf2.Key(sha256.New, pw, saltBytes, iterations, keyLen)
	if err != nil {
		return "", "", err
	}
	return fmt.Sprintf("%d:%s", iterations, hex.EncodeToString(saltBytes)), hex.EncodeToString(dk), nil
}

// parseSalt 解析盐：支持 "iterations:hex" 新格式与旧版纯 hex 格式（按 10 万次处理）。
func parseSalt(s string) (iter int, salt []byte, ok bool) {
	if i := strings.IndexByte(s, ':'); i > 0 {
		if n, err := strconv.Atoi(s[:i]); err == nil && n > 0 {
			b, err := hex.DecodeString(s[i+1:])
			if err != nil {
				return 0, nil, false
			}
			return n, b, true
		}
	}
	b, err := hex.DecodeString(s)
	if err != nil {
		return 0, nil, false
	}
	return legacyIterations, b, true
}

func VerifyPassword(pw, salt, hashHex string) bool {
	iter, saltBytes, ok := parseSalt(salt)
	if !ok {
		return false
	}
	dk, err := pbkdf2.Key(sha256.New, pw, saltBytes, iter, keyLen)
	if err != nil {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(hex.EncodeToString(dk)), []byte(hashHex)) == 1
}

// NeedsRehash 判断存储的盐是否为旧格式（迭代次数与当前不一致）。
// 登录验证成功后调用，命中则用新参数重新哈希并写回。
func NeedsRehash(salt string) bool {
	iter, _, ok := parseSalt(salt)
	return ok && iter != iterations
}

// Session 是登录态载荷。
type Session struct {
	UID      int64  `json:"uid"`
	Username string `json:"u"`
	Exp      int64  `json:"exp"`
	JTI      string `json:"j"` // 会话唯一 ID（随机），用于会话列表与强制下线
}

// NewSession 生成一个新会话（含随机 JTI）。
// ttl 为会话有效期：普通登录 24h；勾选「记住我」时 30 天。
func NewSession(uid int64, username string, ttl time.Duration) *Session {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return &Session{
		UID:      uid,
		Username: username,
		Exp:      time.Now().Add(ttl).Unix(),
		JTI:      hex.EncodeToString(b),
	}
}

// Token 返回 "payload.signature" 形式的 HMAC 签名令牌。
func (s *Session) Token(secret []byte) string {
	payload, _ := json.Marshal(s)
	b := base64.RawURLEncoding.EncodeToString(payload)
	return b + "." + sign(b, secret)
}

// ParseToken 校验签名与过期时间。
func ParseToken(token string, secret []byte) (*Session, bool) {
	parts := strings.Split(token, ".")
	if len(parts) != 2 {
		return nil, false
	}
	if !hmac.Equal([]byte(parts[1]), []byte(sign(parts[0], secret))) {
		return nil, false
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[0])
	if err != nil {
		return nil, false
	}
	var s Session
	if err := json.Unmarshal(payload, &s); err != nil {
		return nil, false
	}
	if s.Exp < time.Now().Unix() {
		return nil, false
	}
	return &s, true
}

// sign 用 HMAC-SHA256 对载荷生成 base64url 签名。
func sign(data string, secret []byte) string {
	mac := hmac.New(func() hash.Hash { return sha256.New() }, secret)
	mac.Write([]byte(data))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}
