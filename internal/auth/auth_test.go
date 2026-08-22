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

package auth

import (
	"crypto/pbkdf2"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"strings"
	"testing"
	"time"
)

func TestHashAndVerify(t *testing.T) {
	salt, hash, err := HashPassword("s3cret-pass")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(salt, ":") {
		t.Fatalf("salt should carry iterations prefix, got %q", salt)
	}
	if !VerifyPassword("s3cret-pass", salt, hash) {
		t.Fatal("correct password rejected")
	}
	if VerifyPassword("wrong-pass", salt, hash) {
		t.Fatal("wrong password accepted")
	}
	if NeedsRehash(salt) {
		t.Fatal("fresh hash should not need rehash")
	}
	if _, _, err := HashPassword(""); err != nil {
		t.Fatalf("empty password should hash: %v", err)
	}
}

// TestLegacyHashCompatibility 模拟旧版（10 万次迭代、纯 hex 盐）存储的哈希：
// 必须能验证通过，且被标记为需要重新哈希（登录后自动升级）。
func TestLegacyHashCompatibility(t *testing.T) {
	salt := make([]byte, saltLen)
	if _, err := rand.Read(salt); err != nil {
		t.Fatal(err)
	}
	dk, err := pbkdf2.Key(sha256.New, "old-password", salt, legacyIterations, keyLen)
	if err != nil {
		t.Fatal(err)
	}
	saltHex := hex.EncodeToString(salt)
	hashHex := hex.EncodeToString(dk)

	if !VerifyPassword("old-password", saltHex, hashHex) {
		t.Fatal("legacy hash should verify with old iterations")
	}
	if VerifyPassword("wrong", saltHex, hashHex) {
		t.Fatal("legacy hash accepted wrong password")
	}
	if !NeedsRehash(saltHex) {
		t.Fatal("legacy salt should be flagged for rehash")
	}
	// 畸形盐不应 panic，且验证失败
	if VerifyPassword("x", "zzz:::", "zzz") {
		t.Fatal("malformed salt accepted")
	}
}

func TestTokenRoundtrip(t *testing.T) {
	secret := []byte("test-secret")
	s := NewSession(1, "admin", time.Hour)
	tok := s.Token(secret)
	parsed, ok := ParseToken(tok, secret)
	if !ok {
		t.Fatal("valid token rejected")
	}
	if parsed.UID != 1 || parsed.Username != "admin" || parsed.JTI != s.JTI {
		t.Fatalf("roundtrip mismatch: %+v", parsed)
	}

	if _, ok := ParseToken(tok, []byte("other-secret")); ok {
		t.Fatal("token signed with wrong secret accepted")
	}
	if _, ok := ParseToken(tok+"x", secret); ok {
		t.Fatal("tampered token accepted")
	}
	expired := NewSession(1, "admin", -time.Minute)
	if _, ok := ParseToken(expired.Token(secret), secret); ok {
		t.Fatal("expired token accepted")
	}
	if _, ok := ParseToken("garbage.not-a-signature", secret); ok {
		t.Fatal("garbage token accepted")
	}
}
