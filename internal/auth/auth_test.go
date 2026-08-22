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
