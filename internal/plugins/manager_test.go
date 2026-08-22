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
	"io"
	"log/slog"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"
)

// mockStore 实现 Store 接口，供管理器单元测试使用。
type mockStore struct {
	installed map[string]bool
	keepalive map[string]bool
}

func newMockStore() *mockStore {
	return &mockStore{installed: map[string]bool{}, keepalive: map[string]bool{}}
}
func (m *mockStore) IsInstalled(name string) bool          { return m.installed[name] }
func (m *mockStore) IsKeepalive(name string) bool          { return m.keepalive[name] }
func (m *mockStore) SetKeepalive(name string, v bool) error { m.keepalive[name] = v; return nil }

func testLogger() *slog.Logger {
	return slog.New(slog.NewTextHandler(io.Discard, nil))
}

func TestIsListening(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	port := ln.Addr().(*net.TCPAddr).Port
	if !isListening("127.0.0.1", port) {
		t.Fatalf("listening port %d not detected", port)
	}
	// 释放后不应再被判定为占用
	closedPort := port
	ln.Close()
	if isListening("127.0.0.1", closedPort) {
		t.Fatalf("closed port %d still detected", closedPort)
	}
}

func TestRotateLog(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "x.log")
	if err := os.WriteFile(p, bytes.Repeat([]byte("a"), 100), 0o644); err != nil {
		t.Fatal(err)
	}
	rotateLog(p, 50) // 超限 → 轮转
	if _, err := os.Stat(p); err == nil {
		t.Fatal("original log should be renamed away")
	}
	if _, err := os.Stat(p + ".1"); err != nil {
		t.Fatal("rotated backup missing")
	}
	rotateLog(p, 50) // 文件不存在 → 应无副作用、不报错
	rotateLog(p+".1", 50)
}

func TestAllocPortSkipsOccupied(t *testing.T) {
	m := NewManager(t.TempDir(), time.Minute, 20000, 20100, newMockStore(), testLogger())
	// 占用池内一个端口
	ln, err := net.Listen("tcp", "127.0.0.1:20055")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()

	p1, err := m.allocPortLocked("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	if p1 == 20055 {
		t.Fatal("allocated an occupied port")
	}
	// 占用后（模拟插件占用），再次分配不得重复
	rt := &Runtime{name: "x", port: p1}
	m.runtimes["x"] = rt
	p2, err := m.allocPortLocked("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	if p2 == p1 {
		t.Fatalf("allocated the same port twice: %d", p2)
	}
}

func TestSameProcess(t *testing.T) {
	cmd := exec.Command("sleep", "30")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer cmd.Process.Kill()

	rt := &Runtime{pid: cmd.Process.Pid}
	rt.startTick, _ = procStartTick(rt.pid)
	if rt.startTick == 0 {
		t.Skip("无法读取进程启动节拍（非 Linux 或 /proc 不可用）")
	}
	if !sameProcess(rt) {
		t.Fatal("live process should match its start tick")
	}
	// 进程退出后：/proc 消失 → 不应再认为同一进程
	if err := cmd.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	_ = cmd.Wait()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		if !sameProcess(rt) {
			break
		}
		time.Sleep(50 * time.Millisecond)
	}
	if sameProcess(rt) {
		t.Fatal("dead process should not match")
	}
}
