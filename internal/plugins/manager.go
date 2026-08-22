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

package plugins

import (
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"micropanel/internal/config"
)

const (
	readinessTimeout = 6 * time.Second
	// maxLogBytes 插件日志单文件上限（超限启动时轮转保留一份 .1）
	maxLogBytes = 20 << 20
)

// Store 是管理器依赖的持久化接口（由 db.DB 实现）。
type Store interface {
	IsInstalled(name string) bool
	IsKeepalive(name string) bool
	SetKeepalive(name string, v bool) error
}

// Port 返回插件监听端口（供网关与 API 层读取）。
func (rt *Runtime) Port() int { return rt.port }

// PID 返回插件进程号。
func (rt *Runtime) PID() int { return rt.pid }

// Bind 返回插件监听地址（网关连接目标；缺省 127.0.0.1）。
func (rt *Runtime) Bind() string {
	if rt.bind == "" {
		return "127.0.0.1"
	}
	return rt.bind
}

// PortMapEntry 对应 port-map.json 中的一条记录。
type PortMapEntry struct {
	Port      int    `json:"port"`
	PID       int    `json:"pid"`
	StartedAt string `json:"started_at"`
}

// Runtime 描述一个正在运行的插件进程。
type Runtime struct {
	name      string
	cmd       *exec.Cmd
	port      int
	pid       int
	bind      string   // manifest.bind（默认 127.0.0.1），网关连接目标
	startTick uint64   // unix 下 /proc/<pid>/stat 的启动节拍，防 PID 复用误杀
	startedAt time.Time
	adopted   bool // 核心启动时认领的残留进程
	lastUse   atomic.Int64
	timer     *time.Timer
}

type Status struct {
	Running   bool   `json:"running"`
	Port      int    `json:"port"`
	PID       int    `json:"pid"`
	StartedAt string `json:"started_at"`
}

type Manager struct {
	Home        string
	Idle        time.Duration
	PortLo      int
	PortHi      int
	store       Store
	log         *slog.Logger
	mu          sync.Mutex
	runtimes    map[string]*Runtime
	portMapPath string
}

func NewManager(home string, idle time.Duration, portLo, portHi int, store Store, logger *slog.Logger) *Manager {
	return &Manager{
		Home:        home,
		Idle:        idle,
		PortLo:      portLo,
		PortHi:      portHi,
		store:       store,
		log:         logger,
		runtimes:    map[string]*Runtime{},
		portMapPath: filepath.Join(home, "etc", "port-map.json"),
	}
}

// SetIdle 动态调整空闲退出时间（设置页保存后调用）。
func (m *Manager) SetIdle(d time.Duration) { m.Idle = d }

// Load 在核心启动时扫描 port-map.json：
// 端口仍被占用的记录直接复用（不杀进程），失效记录清理。
func (m *Manager) Load() {
	m.mu.Lock()
	defer m.mu.Unlock()
	entries := m.readPortMap()
	for name, e := range entries {
		// 按插件 manifest 的 bind 探测（支持 0.0.0.0 / 指定 IP 的插件，避免误判失效）
		bind := "127.0.0.1"
		if mf, err := LoadManifest(filepath.Join(m.Home, "plugins", name)); err == nil && mf.Bind != "" {
			bind = mf.Bind
		}
		if e.Port <= 0 || !isListening(bind, e.Port) {
			m.log.Info("drop stale port-map entry", "plugin", name)
			continue
		}
		rt := &Runtime{name: name, port: e.Port, pid: e.PID, bind: bind, startedAt: time.Now(), adopted: true}
		rt.startTick, _ = procStartTick(e.PID)
		rt.lastUse.Store(time.Now().UnixNano())
		m.runtimes[name] = rt
		m.log.Info("adopted running plugin", "plugin", name, "port", e.Port, "pid", e.PID)
		if !m.store.IsKeepalive(name) {
			m.armIdleLocked(rt)
		}
	}
	m.savePortMapLocked()
}

// Start 冷启动插件进程，等待端口就绪。
func (m *Manager) Start(name string) (*Runtime, error) {
	m.mu.Lock()
	if rt, ok := m.runtimes[name]; ok {
		m.mu.Unlock()
		return rt, nil
	}
	if !m.store.IsInstalled(name) {
		m.mu.Unlock()
		return nil, fmt.Errorf("插件未安装: %s", name)
	}
	pluginDir := filepath.Join(m.Home, "plugins", name)
	mf, err := LoadManifest(pluginDir)
	if err != nil {
		m.mu.Unlock()
		return nil, err
	}
	port, err := m.allocPortLocked(mf.Bind)
	if err != nil {
		m.mu.Unlock()
		return nil, err
	}
	cmdPath := filepath.Join(pluginDir, mf.Command)
	if _, err := os.Stat(cmdPath); err != nil {
		m.mu.Unlock()
		return nil, fmt.Errorf("插件入口不存在: %s", mf.Command)
	}
	logPath := filepath.Join(m.Home, "logs", "plugins", name+".log")
	os.MkdirAll(filepath.Dir(logPath), 0o755)
	rotateLog(logPath, maxLogBytes)
	logFile, err := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		m.mu.Unlock()
		return nil, err
	}
	// 日志头部（pid 需在 cmd.Start 之后取，才是插件进程自身的 PID）

	cmd := exec.Command(cmdPath, mf.Args...)
	cmd.Dir = pluginDir
	cmd.Env = append(os.Environ(),
		"PLUGIN_PORT="+strconv.Itoa(port),
		"PLUGIN_BIND="+mf.Bind,
		"PLUGIN_NAME="+name,
		"PANEL_HOME="+m.Home,
		"MICROPANEL_VERSION="+config.Version,
	)
	cmd.Stdout = logFile
	cmd.Stderr = logFile

	if err := cmd.Start(); err != nil {
		logFile.Close()
		m.mu.Unlock()
		return nil, fmt.Errorf("启动插件进程失败: %w", err)
	}
	fmt.Fprintf(logFile, "\n=== [%s] start, port=%d, pid=%d, %s ===\n",
		name, port, cmd.Process.Pid, time.Now().Format(time.RFC3339))
	rt := &Runtime{name: name, cmd: cmd, port: port, pid: cmd.Process.Pid, bind: mf.Bind, startedAt: time.Now()}
	rt.startTick, _ = procStartTick(rt.pid)
	rt.lastUse.Store(time.Now().UnixNano())
	m.runtimes[name] = rt
	m.savePortMapLocked()
	m.mu.Unlock()

	// 等端口就绪（冷启动等待窗口）
	if err := waitPort(mf.Bind, port, readinessTimeout); err != nil {
		m.mu.Lock()
		if cur, ok := m.runtimes[name]; ok && cur == rt {
			delete(m.runtimes, name)
			m.savePortMapLocked()
		}
		m.mu.Unlock()
		killProc(rt)
		_ = cmd.Wait()
		tail, _ := tailLog(logPath, 20)
		return nil, fmt.Errorf("插件启动超时（%v）：%s", readinessTimeout, strings.TrimSpace(tail))
	}

	// 进程退出时立刻清理运行条目：防止死进程残留（网关持续 502），
	// 也避免空闲计时器对已死进程触发 killProc 而误伤被复用 PID 的无辜进程。
	go func() {
		_ = cmd.Wait()
		m.mu.Lock()
		if cur, ok := m.runtimes[name]; ok && cur == rt {
			delete(m.runtimes, name)
			if cur.timer != nil {
				cur.timer.Stop()
			}
			m.savePortMapLocked()
		}
		m.mu.Unlock()
		m.log.Info("plugin process exited, entry cleaned", "plugin", name, "pid", rt.pid)
	}()

	m.mu.Lock()
	if !m.store.IsKeepalive(name) {
		m.armIdleLocked(rt)
	}
	m.mu.Unlock()
	m.log.Info("plugin started", "plugin", name, "port", port, "pid", rt.pid)
	return rt, nil
}

// Stop 停止插件进程并清理端口映射。
func (m *Manager) Stop(name string) error {
	m.mu.Lock()
	rt, ok := m.runtimes[name]
	if !ok {
		m.mu.Unlock()
		return fmt.Errorf("插件未在运行: %s", name)
	}
	delete(m.runtimes, name)
	if rt.timer != nil {
		rt.timer.Stop()
	}
	m.savePortMapLocked()
	m.mu.Unlock()
	killProc(rt)
	m.log.Info("plugin stopped", "plugin", name, "pid", rt.pid)
	return nil
}

// Restart 重启插件（保活设置不受影响）。
func (m *Manager) Restart(name string) error {
	_ = m.Stop(name)
	_, err := m.Start(name)
	return err
}

// Touch 记录插件活跃时间并重置空闲计时器（事件驱动，无常驻轮询）。
func (m *Manager) Touch(name string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	rt, ok := m.runtimes[name]
	if !ok || rt == nil {
		return
	}
	rt.lastUse.Store(time.Now().UnixNano())
	if rt.timer != nil && !m.store.IsKeepalive(name) && m.Idle > 0 {
		rt.timer.Reset(m.Idle)
	}
}

// Status 返回插件运行状态。
func (m *Manager) Status(name string) Status {
	m.mu.Lock()
	defer m.mu.Unlock()
	if rt, ok := m.runtimes[name]; ok {
		return Status{Running: true, Port: rt.port, PID: rt.pid,
			StartedAt: rt.startedAt.Format(time.RFC3339)}
	}
	return Status{Running: false}
}

// ApplyKeepalive 在运行中切换保活状态：
// 开启则取消空闲计时器；关闭则按当前空闲策略重新武装。
func (m *Manager) ApplyKeepalive(name string, enabled bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	rt, ok := m.runtimes[name]
	if !ok || rt == nil {
		return
	}
	if enabled {
		if rt.timer != nil {
			rt.timer.Stop()
			rt.timer = nil
		}
		m.log.Info("plugin keepalive enabled", "plugin", name)
	} else {
		if rt.timer == nil {
			m.armIdleLocked(rt)
			m.log.Info("plugin keepalive disabled, idle timer armed", "plugin", name)
		}
	}
}

// Shutdown 处理核心退出：仅停止未开启保活的插件，保活插件进程保留（核心重启后由 Load 复用）。
func (m *Manager) Shutdown() {
	m.mu.Lock()
	var victims []string
	for name, rt := range m.runtimes {
		if !m.store.IsKeepalive(name) {
			victims = append(victims, name)
		} else {
			m.log.Info("keep plugin alive across core shutdown", "plugin", name, "pid", rt.pid)
		}
	}
	m.mu.Unlock()
	for _, n := range victims {
		_ = m.Stop(n)
	}
}

// ---------- 内部 ----------

func (m *Manager) armIdleLocked(rt *Runtime) {
	if m.Idle <= 0 {
		return
	}
	rt.timer = time.AfterFunc(m.Idle, func() {
		m.mu.Lock()
		cur, ok := m.runtimes[rt.name]
		if !ok || cur != rt {
			m.mu.Unlock()
			return
		}
		if m.store.IsKeepalive(rt.name) {
			m.mu.Unlock()
			return
		}
		if time.Since(time.Unix(0, rt.lastUse.Load())) < m.Idle {
			rt.timer.Reset(m.Idle) // 刚被 Touch 过，顺延
			m.mu.Unlock()
			return
		}
		delete(m.runtimes, rt.name)
		m.savePortMapLocked()
		m.mu.Unlock()
		killProc(rt)
		m.log.Info("plugin idle-exited, memory released", "plugin", rt.name)
	})
}

// allocPortLocked 在端口池中寻找未被本管理器与系统占用的端口。
// bind 用于探测：插件监听 0.0.0.0 / 指定 IP 时也检查对应地址，避免端口冲突。
func (m *Manager) allocPortLocked(bind string) (int, error) {
	if bind == "" {
		bind = "127.0.0.1"
	}
	for p := m.PortLo; p <= m.PortHi; p++ {
		inUse := false
		for _, rt := range m.runtimes {
			if rt.port == p {
				inUse = true
				break
			}
		}
		if inUse || isListening("127.0.0.1", p) || (bind != "127.0.0.1" && isListening(bind, p)) {
			continue
		}
		return p, nil
	}
	return 0, errors.New("插件端口池已耗尽")
}

// readPortMap 读取 port-map.json；文件不存在或解析失败时返回空映射。
func (m *Manager) readPortMap() map[string]PortMapEntry {
	out := map[string]PortMapEntry{}
	data, err := os.ReadFile(m.portMapPath)
	if err != nil {
		return out
	}
	_ = json.Unmarshal(data, &out)
	return out
}

// savePortMapLocked 把当前运行中的插件写入 port-map.json（临时文件 + 原子替换）。
func (m *Manager) savePortMapLocked() {
	out := map[string]PortMapEntry{}
	for name, rt := range m.runtimes {
		out[name] = PortMapEntry{
			Port:      rt.port,
			PID:       rt.pid,
			StartedAt: rt.startedAt.Format(time.RFC3339),
		}
	}
	data, _ := json.MarshalIndent(out, "", "  ")
	dir := filepath.Dir(m.portMapPath)
	os.MkdirAll(dir, 0o755)
	tmp := m.portMapPath + ".tmp"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return
	}
	os.Rename(tmp, m.portMapPath) // 原子替换
}

// waitPort 在超时时间内轮询 TCP 端口，等待插件进程就绪。
func waitPort(bind string, port int, timeout time.Duration) error {
	addr := net.JoinHostPort(bind, strconv.Itoa(port))
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", addr, 200*time.Millisecond)
		if err == nil {
			conn.Close()
			return nil
		}
		time.Sleep(100 * time.Millisecond)
	}
	return errors.New("timeout waiting for plugin port")
}

// isListening 探测指定地址的 TCP 端口是否已被监听。
func isListening(host string, port int) bool {
	conn, err := net.DialTimeout("tcp", net.JoinHostPort(host, strconv.Itoa(port)), 300*time.Millisecond)
	if err != nil {
		return false
	}
	conn.Close()
	return true
}

// killProc 向插件进程发送 SIGTERM，最多等待 3 秒后 SIGKILL。
// 发送前校验进程启动时间：PID 已被系统回收复用时绝不向无关进程发信号。
// 使用 os.FindProcess + Signal（跨平台，Windows 亦可编译运行）。
func killProc(rt *Runtime) {
	if rt.pid <= 0 {
		return
	}
	proc, err := os.FindProcess(rt.pid)
	if err != nil {
		return
	}
	if !sameProcess(rt) {
		return // 原进程已不存在 / PID 被复用
	}
	_ = proc.Signal(syscall.SIGTERM)
	for i := 0; i < 30; i++ { // 最多等 3 秒优雅退出
		time.Sleep(100 * time.Millisecond)
		if !sameProcess(rt) {
			return // 原进程已退出（或 PID 被复用），不再处理
		}
	}
	_ = proc.Signal(syscall.SIGKILL)
}

// sameProcess 确认 PID 仍是 Runtime 当初拉起的那个进程：
// 优先比较 /proc 启动节拍（unix）；无法获取时退化为存活探测。
func sameProcess(rt *Runtime) bool {
	if rt.startTick > 0 {
		if tick, ok := procStartTick(rt.pid); !ok || tick != rt.startTick {
			return false
		}
		return true
	}
	return processAlive(rt.pid)
}

// tailLog 读取日志文件最后 n 行（启动失败诊断用）。
func tailLog(path string, n int) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	return strings.Join(lines, "\n"), nil
}

// rotateLog 简单轮转：文件超过 maxBytes 时把当前文件改名为 .1（覆盖旧的 .1）。
// 在每次打开日志前调用，防止日志无限增长。
func rotateLog(path string, maxBytes int64) {
	if fi, err := os.Stat(path); err == nil && fi.Size() > maxBytes {
		_ = os.Rename(path, path+".1")
	}
}
