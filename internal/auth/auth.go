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
	"hash"
	"strings"
	"time"
)

const (
	iterations = 100_000
	keyLen     = 32
	saltLen    = 16
	CookieName = "mp_session"
	sessionTTL = 24 * time.Hour
)

// HashPassword 生成 salt 与 PBKDF2 哈希（均为 hex 字符串）。
func HashPassword(pw string) (saltHex, hashHex string, err error) {
	salt := make([]byte, saltLen)
	if _, err = rand.Read(salt); err != nil {
		return "", "", err
	}
	// Go 1.25+ 的 pbkdf2.Key：签名变为 Key(h func() Hash, password string, salt, iter, keyLen) ([]byte, error)
	dk, err := pbkdf2.Key(sha256.New, pw, salt, iterations, keyLen)
	if err != nil {
		return "", "", err
	}
	return hex.EncodeToString(salt), hex.EncodeToString(dk), nil
}

func VerifyPassword(pw, saltHex, hashHex string) bool {
	salt, err := hex.DecodeString(saltHex)
	if err != nil {
		return false
	}
	dk, err := pbkdf2.Key(sha256.New, pw, salt, iterations, keyLen)
	if err != nil {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(hex.EncodeToString(dk)), []byte(hashHex)) == 1
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
