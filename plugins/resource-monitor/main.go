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

// 资源监控插件。
//
// 读取 /proc 伪文件系统获取 CPU、内存/交换分区、负载、磁盘、进程列表、
// 网络流量信息，以独立进程运行，由面板核心按需冷启动。
//
// 注意：前端页面里的 AJAX 一律使用【相对路径】api/...，
//
//	经面板网关转发后即为 /p/<插件名>/api/...。
package main

import (
	"bufio"
	"embed"
	"encoding/json"
	"errors"
	"log"
	"net/http"
	"os"
	"os/signal"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

//go:embed web
var webFS embed.FS

// 网络流量采样（用于计算实时速率）
var netState struct {
	sync.Mutex
	lastRx, lastTx uint64
	lastAt         time.Time
}

func main() {
	port := os.Getenv("PLUGIN_PORT")
	bind := envOr("PLUGIN_BIND", "127.0.0.1")
	if port == "" {
		port = "19002"
	}
	mux := http.NewServeMux()
	mux.HandleFunc("GET /", handleIndex)          // 监控页面
	mux.HandleFunc("GET /api/stats", handleStats) // 统计数据 JSON

	addr := bind + ":" + port
	server := &http.Server{Addr: addr, Handler: mux}
	go func() {
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGTERM, syscall.SIGINT)
		<-sig
		log.Printf("[resource-monitor] 收到退出信号，正在关闭")
		server.Close()
	}()
	log.Printf("[resource-monitor] listening on %s", addr)
	if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}

// ---------- 页面 ----------

// handleIndex 返回内嵌的插件页面
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

// ---------- 数据采集 ----------

// handleStats 汇总各项系统指标
func handleStats(w http.ResponseWriter, r *http.Request) {
	cpu, _ := readCPUPercent()
	memTotal, memAvail := readMemInfo()
	swapTotal, swapFree := readSwapInfo()
	loads, _ := readLoadAvg()
	uptime, _ := readUptime()
	disk := readDisk()
	procs := readProcesses()
	net := readNetwork()

	// 内存使用率
	memUsed := memTotal - memAvail
	memPercent := 0.0
	if memTotal > 0 {
		memPercent = float64(memUsed) / float64(memTotal) * 100
	}
	// 交换分区使用率
	swapUsed := swapTotal - swapFree
	swapPercent := 0.0
	if swapTotal > 0 {
		swapPercent = float64(swapUsed) / float64(swapTotal) * 100
	}

	writeJSON(w, map[string]any{
		"cpu_percent": cpu,
		"mem": map[string]any{
			"total": memTotal, "used": memUsed, "available": memAvail, "percent": memPercent,
		},
		"swap": map[string]any{
			"total": swapTotal, "used": swapUsed, "free": swapFree, "percent": swapPercent,
		},
		"load":      loads,
		"uptime":    uptime,
		"hostname":  hostname(),
		"disk":      disk,
		"processes": procs,
		"network":   net,
	})
}

// readCPUPercent 通过两次读取 /proc/stat 计算 CPU 使用率（间隔 300ms）
func readCPUPercent() (float64, error) {
	prevIdle, prevTotal, err := cpuTimes()
	if err != nil {
		return 0, err
	}
	time.Sleep(300 * time.Millisecond)
	curIdle, curTotal, err := cpuTimes()
	if err != nil {
		return 0, err
	}
	dIdle := curIdle - prevIdle
	dTotal := curTotal - prevTotal
	if dTotal <= 0 {
		return 0, nil
	}
	return (1 - float64(dIdle)/float64(dTotal)) * 100, nil
}

// cpuTimes 从 /proc/stat 第一行解析 idle 与总 CPU 时间
func cpuTimes() (idle, total uint64, err error) {
	f, err := os.Open("/proc/stat")
	if err != nil {
		return 0, 0, err
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	if !sc.Scan() {
		return 0, 0, errors.New("无法读取 /proc/stat")
	}
	fields := strings.Fields(sc.Text())
	if len(fields) < 5 || fields[0] != "cpu" {
		return 0, 0, errors.New("/proc/stat 格式异常")
	}
	var vals []uint64
	for _, s := range fields[1:] {
		v, err := strconv.ParseUint(s, 10, 64)
		if err != nil {
			return 0, 0, err
		}
		vals = append(vals, v)
	}
	// 标准 idle = idle + iowait（第 4、5 个字段）
	idle = vals[3] + vals[4]
	for _, v := range vals {
		total += v
	}
	return idle, total, nil
}

// readMemInfo 从 /proc/meminfo 读取内存总量与可用量（字节）
func readMemInfo() (total, available uint64) {
	f, err := os.Open("/proc/meminfo")
	if err != nil {
		return 0, 0
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Text()
		switch {
		case strings.HasPrefix(line, "MemTotal:"):
			total = parseKB(line)
		case strings.HasPrefix(line, "MemAvailable:"):
			available = parseKB(line)
		}
	}
	return total, available
}

// readSwapInfo 从 /proc/meminfo 读取交换分区总量与空闲（字节）
func readSwapInfo() (total, free uint64) {
	f, err := os.Open("/proc/meminfo")
	if err != nil {
		return 0, 0
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := sc.Text()
		switch {
		case strings.HasPrefix(line, "SwapTotal:"):
			total = parseKB(line)
		case strings.HasPrefix(line, "SwapFree:"):
			free = parseKB(line)
		}
	}
	return total, free
}

// parseKB 解析 "MemTotal:       16384000 kB" 这类行，返回字节数
func parseKB(line string) uint64 {
	fields := strings.Fields(line)
	if len(fields) < 2 {
		return 0
	}
	kb, _ := strconv.ParseUint(fields[1], 10, 64)
	return kb * 1024
}

// readLoadAvg 返回 1/5/15 分钟负载
func readLoadAvg() ([3]float64, error) {
	var out [3]float64
	data, err := os.ReadFile("/proc/loadavg")
	if err != nil {
		return out, err
	}
	fields := strings.Fields(string(data))
	if len(fields) < 3 {
		return out, errors.New("/proc/loadavg 格式异常")
	}
	for i := 0; i < 3; i++ {
		out[i], _ = strconv.ParseFloat(fields[i], 64)
	}
	return out, nil
}

// readUptime 返回系统运行秒数
func readUptime() (float64, error) {
	data, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return 0, err
	}
	fields := strings.Fields(string(data))
	if len(fields) < 1 {
		return 0, errors.New("/proc/uptime 格式异常")
	}
	return strconv.ParseFloat(fields[0], 64)
}

// readDisk 返回根分区的容量信息（statfs 系统调用）
func readDisk() map[string]any {
	var st syscall.Statfs_t
	if err := syscall.Statfs("/", &st); err != nil {
		return map[string]any{"error": err.Error()}
	}
	total := st.Blocks * uint64(st.Bsize)
	free := st.Bavail * uint64(st.Bsize)
	used := total - free
	percent := 0.0
	if total > 0 {
		percent = float64(used) / float64(total) * 100
	}
	return map[string]any{"total": total, "used": used, "free": free, "percent": percent}
}

// ---------- 进程列表 ----------

type procInfo struct {
	PID  int     `json:"pid"`
	Name string  `json:"name"`
	CPU  float64 `json:"cpu"`
	Mem  uint64  `json:"mem"` // 常驻内存字节
}

// readProcesses 扫描 /proc，返回按 CPU 占用排序的前 15 个进程
func readProcesses() []procInfo {
	dirs, err := os.ReadDir("/proc")
	if err != nil {
		return nil
	}
	type sample struct {
		pid   int
		name  string
		ticks uint64
		rss   uint64
	}
	snap := func() map[int]sample {
		out := map[int]sample{}
		for _, d := range dirs {
			if !d.IsDir() || d.Name()[0] < '0' || d.Name()[0] > '9' {
				continue
			}
			pid, err := strconv.Atoi(d.Name())
			if err != nil {
				continue
			}
			data, err := os.ReadFile("/proc/" + d.Name() + "/stat")
			if err != nil {
				continue
			}
			// /proc/pid/stat: pid (comm) state ...（comm 可能含空格/括号，从最后一个 ')' 切分）
			idx := strings.LastIndexByte(string(data), ')')
			if idx < 0 {
				continue
			}
			fields := strings.Fields(string(data[idx+2:]))
			// fields 从 state 开始：utime=field[11], stime=field[12], rss=field[21]
			if len(fields) < 22 {
				continue
			}
			utime, _ := strconv.ParseUint(fields[11], 10, 64)
			stime, _ := strconv.ParseUint(fields[12], 10, 64)
			rss, _ := strconv.ParseUint(fields[21], 10, 64)
			comm := data[strings.IndexByte(string(data), '(')+1 : idx]
			out[pid] = sample{pid: pid, name: string(comm), ticks: utime + stime, rss: rss * 4096}
		}
		return out
	}

	first := snap()
	time.Sleep(400 * time.Millisecond)
	second := snap()

	// 全局 CPU 时间差（用于计算占比，近似值）
	totalTicks := uint64(0)
	procs := make([]procInfo, 0, len(second))
	for pid, s2 := range second {
		s1, ok := first[pid]
		if !ok {
			continue
		}
		d := s2.ticks - s1.ticks
		totalTicks += d
		procs = append(procs, procInfo{PID: pid, Name: s2.name, CPU: 0, Mem: s2.rss})
		procs[len(procs)-1].CPU = float64(d) // 先暂存 tick 差，下面统一换算
	}
	// 排序：tick 差降序
	sort.Slice(procs, func(i, j int) bool { return procs[i].CPU > procs[j].CPU })
	if totalTicks > 0 {
		for i := range procs {
			procs[i].CPU = procs[i].CPU / float64(totalTicks) * 100
		}
	}
	if len(procs) > 15 {
		procs = procs[:15]
	}
	return procs
}

// ---------- 网络流量 ----------

// readNetwork 读取 /proc/net/dev 汇总各网卡收发字节数，并计算实时速率
func readNetwork() map[string]any {
	f, err := os.Open("/proc/net/dev")
	if err != nil {
		return map[string]any{"error": err.Error()}
	}
	defer f.Close()
	var totalRx, totalTx uint64
	ifaces := map[string]any{}
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if !strings.Contains(line, ":") {
			continue // 跳过表头
		}
		parts := strings.SplitN(line, ":", 2)
		if len(parts) != 2 {
			continue
		}
		name := strings.TrimSpace(parts[0])
		fields := strings.Fields(parts[1])
		if len(fields) < 9 {
			continue
		}
		rx, _ := strconv.ParseUint(fields[0], 10, 64)
		tx, _ := strconv.ParseUint(fields[8], 10, 64)
		totalRx += rx
		totalTx += tx
		ifaces[name] = map[string]uint64{"rx": rx, "tx": tx}
	}

	// 实时速率（字节/秒）
	netState.Lock()
	defer netState.Unlock()
	now := time.Now()
	rxRate, txRate := 0.0, 0.0
	if !netState.lastAt.IsZero() && netState.lastAt.Before(now) {
		secs := now.Sub(netState.lastAt).Seconds()
		if secs > 0 {
			rxRate = float64(totalRx-netState.lastRx) / secs
			txRate = float64(totalTx-netState.lastTx) / secs
		}
	}
	netState.lastRx, netState.lastTx, netState.lastAt = totalRx, totalTx, now

	return map[string]any{
		"total_rx": totalRx, "total_tx": totalTx,
		"rx_rate": rxRate, "tx_rate": txRate,
		"interfaces": ifaces,
	}
}

func hostname() string {
	h, _ := os.Hostname()
	return h
}

// ---------- 工具 ----------

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	json.NewEncoder(w).Encode(v)
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
