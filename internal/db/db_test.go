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

package db

import (
	"os"
	"path/filepath"
	"testing"
)

func openTestDB(t *testing.T) *DB {
	t.Helper()
	d, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return d
}

func TestOpenAndUserCRUD(t *testing.T) {
	d := openTestDB(t)
	defer d.Close()

	if ok, _ := d.HasAdmin(); ok {
		t.Fatal("fresh DB should have no admin")
	}
	if err := d.CreateUser(User{Username: "admin", PasswordHash: "h", Salt: "s"}); err != nil {
		t.Fatal(err)
	}
	if err := d.CreateUser(User{Username: "admin", PasswordHash: "h2", Salt: "s2"}); err == nil {
		t.Fatal("duplicate username accepted")
	}
	u, err := d.GetUserByName("admin")
	if err != nil || u.Username != "admin" {
		t.Fatalf("GetUserByName failed: %v %+v", err, u)
	}
	if ok, _ := d.HasAdmin(); !ok {
		t.Fatal("HasAdmin should be true after CreateUser")
	}
	if err := d.UpdatePassword("admin", "newhash", "newsalt"); err != nil {
		t.Fatal(err)
	}
	if u, _ := d.GetUserByName("admin"); u.PasswordHash != "newhash" || u.Salt != "newsalt" {
		t.Fatal("UpdatePassword did not persist")
	}
}

func TestPluginLifecycle(t *testing.T) {
	d := openTestDB(t)
	defer d.Close()

	if d.IsInstalled("hello") {
		t.Fatal("fresh DB should not have plugins")
	}
	if err := d.UpsertPlugin(PluginRecord{Name: "hello", Title: "Hello", Version: "0.1.0"}); err != nil {
		t.Fatal(err)
	}
	if !d.IsInstalled("hello") {
		t.Fatal("plugin should be installed after upsert")
	}
	if err := d.SetKeepalive("hello", true); err != nil {
		t.Fatal(err)
	}
	if !d.IsKeepalive("hello") {
		t.Fatal("keepalive flag not persisted")
	}
	recs, _ := d.ListPlugins()
	if len(recs) != 1 || recs[0].Name != "hello" {
		t.Fatalf("ListPlugins wrong: %+v", recs)
	}
	// 升级：同名单次 upsert 不重复
	if err := d.UpsertPlugin(PluginRecord{Name: "hello", Title: "Hello v2", Version: "0.2.0"}); err != nil {
		t.Fatal(err)
	}
	recs, _ = d.ListPlugins()
	if len(recs) != 1 || recs[0].Version != "0.2.0" {
		t.Fatalf("upsert should update in place: %+v", recs)
	}
	if err := d.DeletePlugin("hello"); err != nil {
		t.Fatal(err)
	}
	if d.IsInstalled("hello") {
		t.Fatal("plugin should be uninstalled")
	}
}

func TestSessions(t *testing.T) {
	d := openTestDB(t)
	defer d.Close()

	if err := d.CreateSession(Session{TokenHash: "abc", JTI: "j1", Username: "admin"}); err != nil {
		t.Fatal(err)
	}
	if err := d.CreateSession(Session{TokenHash: "def", JTI: "j2", Username: "admin"}); err != nil {
		t.Fatal(err)
	}
	if _, found, _ := d.GetSessionByTokenHash("abc"); !found {
		t.Fatal("session lookup failed")
	}
	// 单账号单会话：保留 j1，踢掉 j2
	n, err := d.RevokeOtherSessions("admin", "j1")
	if err != nil || n != 1 {
		t.Fatalf("RevokeOtherSessions = %d, %v; want 1", n, err)
	}
	s, found, _ := d.GetSessionByTokenHash("def")
	if !found || !s.Revoked {
		t.Fatal("revoked session should be marked")
	}
	// 按令牌指纹吊销（logout）
	if err := d.RevokeSessionByTokenHash("abc"); err != nil {
		t.Fatal(err)
	}
	if s, _, _ := d.GetSessionByTokenHash("abc"); !s.Revoked {
		t.Fatal("logout should revoke by token hash")
	}
}

func TestBackupFallbackAndTmpCleanup(t *testing.T) {
	home := t.TempDir()
	d, err := Open(home)
	if err != nil {
		t.Fatal(err)
	}
	if err := d.CreateUser(User{Username: "admin", PasswordHash: "h", Salt: "s"}); err != nil {
		t.Fatal(err)
	}
	d.Close()

	// 主文件损坏 → Open 应回退到 .bak
	if err := os.WriteFile(filepath.Join(home, "data", "panel.json"), []byte("{corrupt"), 0o600); err != nil {
		t.Fatal(err)
	}
	d2, err := Open(home)
	if err != nil {
		t.Fatalf("should fall back to .bak: %v", err)
	}
	if ok, _ := d2.HasAdmin(); !ok {
		t.Fatal("admin record lost after .bak fallback")
	}
	d2.Close()

	// 残留 .tmp 应在下次 Open 时被清理
	tmpPath := filepath.Join(home, "data", "panel.json.tmp")
	if err := os.WriteFile(tmpPath, []byte("stale"), 0o600); err != nil {
		t.Fatal(err)
	}
	d3, err := Open(home)
	if err != nil {
		t.Fatal(err)
	}
	d3.Close()
	if _, err := os.Stat(tmpPath); !os.IsNotExist(err) {
		t.Fatal(".tmp should be removed on Open")
	}
}
