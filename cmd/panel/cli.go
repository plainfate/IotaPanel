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

package main

// 面板命令行工具（风格与主流面板相近）：
//   panel start | stop | restart | status | log | version | help
// systemd 安装时通过 systemctl 管理；非 systemd 环境用进程信号 + 分离式重拉。

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"iotapanel/internal/config"
)

const serviceName = "iotapanel"

// runCLI 处理 start/stop/restart/status/log 等子命令后退出。
func runCLI(args []string) {
	if len(args) == 0 {
		printCLIHelp()
		return
	}
	switch args[0] {
	case "start":
		cliStart()
	case "stop":
		cliStop()
	case "restart":
		cliRestart()
	case "uninstall":
		cliUninstall()
	case "status":
		cliStatus()
	case "log":
		cliLog(args)
	case "help", "-h", "--help":
		printCLIHelp()
	default:
		fmt.Printf("未知命令: %s\n\n", args[0])
		printCLIHelp()
	}
}

func printCLIHelp() {
	fmt.Println(`IotaPanel 面板控制命令

用法:
  panel start      启动面板（systemd 安装时走 systemctl）
  panel stop       停止面板（保留保活插件进程）
  panel restart    重启面板
  panel status     查看面板状态（进程/端口/插件）
  panel log        查看核心日志（panel log -n 100 指定行数）
  panel uninstall  卸载面板（停止服务、移除 systemd 与命令，数据保留）
  panel version    显示版本
  panel help       显示帮助

服务名: ` + serviceName + `（systemd: systemctl status ` + serviceName + `）`)
}

// cliUninstall 卸载面板：停止服务、移除 systemd 单元与命令软链，数据目录保留。
func cliUninstall() {
	home := resolveHome()
	if home == "" {
		home = "/data/panel"
	}
	fmt.Println("即将卸载 IotaPanel")
	fmt.Println("安装目录:", home)
	fmt.Print("确认卸载？（停止面板并移除 systemd 服务，数据目录将保留）[y/N]: ")
	var ans string
	fmt.Scanln(&ans)
	if !strings.EqualFold(strings.TrimSpace(ans), "y") && !strings.EqualFold(strings.TrimSpace(ans), "yes") {
		fmt.Println("已取消")
		return
	}
	cliStop()
	if hasSystemd() {
		_ = runSystemctl("stop")
		_ = exec.Command("systemctl", "disable", serviceName).Run()
		_ = os.Remove("/etc/systemd/system/" + serviceName + ".service")
		_ = exec.Command("systemctl", "daemon-reload").Run()
		fmt.Println("已移除 systemd 服务")
	}
	_ = os.Remove("/usr/local/bin/panel") // 命令软链
	fmt.Printf("✅ 已卸载。数据保留在 %s（彻底删除请执行: rm -rf %s）\n", home, home)
}

// hasSystemd 判断是否通过 systemd 管理面板。
func hasSystemd() bool {
	_, err := os.Stat("/etc/systemd/system/" + serviceName + ".service")
	return err == nil
}

// runSystemctl 执行 systemctl 子命令。
func runSystemctl(action string) error {
	cmd := exec.Command("systemctl", action, serviceName)
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	return cmd.Run()
}

// findPanelPIDs 返回当前运行中的面板进程 PID（排除自身）。
func findPanelPIDs() []int {
	out, err := exec.Command("pgrep", "-x", "panel").Output()
	if err != nil {
		return nil
	}
	self := os.Getpid()
	var pids []int
	for _, f := range strings.Fields(string(out)) {
		if pid, err := strconv.Atoi(f); err == nil && pid != self {
			pids = append(pids, pid)
		}
	}
	return pids
}

func cliStart() {
	if hasSystemd() {
		if err := runSystemctl("start"); err != nil {
			fmt.Println("启动失败:", err)
			os.Exit(1)
		}
		fmt.Println("✅ 已启动（systemd）")
		return
	}
	if len(findPanelPIDs()) > 0 {
		fmt.Println("面板已在运行")
		return
	}
	// 恢复 PANEL_HOME：环境变量 > 运行中进程 > 二进制位置推导 > 标记文件（兜底）
	// 优先级顺序保证标准布局（<安装目录>/bin/panel）不被其它实例留下的
	// /tmp/iotapanel-home 标记误导到错误的安装目录。
	if os.Getenv("PANEL_HOME") == "" {
		if home := resolveHome(); home != "" {
			os.Setenv("PANEL_HOME", home)
		}
	}
	// 分离式启动：nohup + 后台运行，输出到日志文件；继承 PANEL_HOME（若有）
	exe, err := os.Executable()
	if err != nil {
		fmt.Println("无法定位自身二进制:", err)
		os.Exit(1)
	}
	logFile, _ := os.OpenFile("/tmp/iotapanel.log", os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	cmd := exec.Command("nohup", exe, "serve")
	cmd.Env = os.Environ()
	cmd.Stdout, cmd.Stderr = logFile, logFile
	if err := cmd.Start(); err != nil {
		fmt.Println("启动失败:", err)
		os.Exit(1)
	}
	fmt.Println("✅ 已启动（非 systemd，日志: /tmp/iotapanel.log）")
}

func cliStop() {
	if hasSystemd() {
		if err := runSystemctl("stop"); err != nil {
			fmt.Println("停止失败:", err)
			os.Exit(1)
		}
		fmt.Println("🛑 已停止（systemd）")
		return
	}
	pids := findPanelPIDs()
	if len(pids) == 0 {
		fmt.Println("面板未在运行")
		return
	}
	for _, pid := range pids {
		if err := exec.Command("kill", "-TERM", strconv.Itoa(pid)).Run(); err == nil {
			fmt.Printf("🛑 已向面板进程 %d 发送停止信号\n", pid)
		}
	}
}

func cliRestart() {
	if hasSystemd() {
		if err := runSystemctl("restart"); err != nil {
			fmt.Println("重启失败:", err)
			os.Exit(1)
		}
		fmt.Println("✅ 已重启（systemd）")
		return
	}
	// 先读取旧进程的真实 PANEL_HOME（旧进程还在运行时），
	// 保证重拉的子进程继续使用同一安装目录
	if home := resolveHome(); home != "" {
		os.Setenv("PANEL_HOME", home)
	}
	cliStop()
	time.Sleep(800 * time.Millisecond) // 等旧进程释放端口
	cliStart()
}

// runningPanelEnv 从正在运行的面板进程读取其环境变量（PANEL_HOME/LISTEN_ADDR 等），
// 比从二进制位置推导更准确。
func runningPanelEnv() map[string]string {
	pids := findPanelPIDs()
	if len(pids) == 0 {
		return nil
	}
	data, err := os.ReadFile(fmt.Sprintf("/proc/%d/environ", pids[0]))
	if err != nil {
		return nil
	}
	env := map[string]string{}
	for _, kv := range strings.Split(string(data), "\x00") {
		if k, v, ok := strings.Cut(kv, "="); ok {
			env[k] = v
		}
	}
	return env
}

// resolveHome 确定安装目录：运行中进程环境 > 自身环境变量 > 二进制位置推导。
func resolveHome() string {
	if env := runningPanelEnv(); env != nil && env["PANEL_HOME"] != "" {
		return env["PANEL_HOME"]
	}
	if home := os.Getenv("PANEL_HOME"); home != "" {
		return home
	}
	exe, err := os.Executable()
	if err == nil {
		if dir := filepath.Dir(filepath.Dir(exe)); filepath.Base(filepath.Dir(exe)) == "bin" {
			return dir
		}
	}
	// 兜底：二进制不在 <安装目录>/bin 布局时，用上次运行留下的标记文件。
	// 只采信看起来是真实面板安装的目录（含 etc/.env），防本地用户伪造标记劫持 root 面板。
	if data, err := os.ReadFile("/tmp/iotapanel-home"); err == nil {
		if home := strings.TrimSpace(string(data)); home != "" {
			if _, err := os.Stat(filepath.Join(home, "etc", ".env")); err == nil {
				return home
			}
		}
	}
	return ""
}

func cliStatus() {
	fmt.Printf("IotaPanel %s\n", config.Version)
	home := resolveHome()
	// 运行中进程的真实监听地址优先
	env := runningPanelEnv()
	listen := ""
	if env != nil {
		listen = env["LISTEN_ADDR"]
	}
	if listen == "" && home != "" {
		// 读 .env 里的监听地址
		if data, err := os.ReadFile(filepath.Join(home, "etc", ".env")); err == nil {
			sc := bufio.NewScanner(strings.NewReader(string(data)))
			for sc.Scan() {
				if k, v, ok := strings.Cut(strings.TrimSpace(sc.Text()), "="); ok && strings.TrimSpace(k) == "LISTEN_ADDR" {
					listen = strings.Trim(strings.TrimSpace(v), `"'`)
				}
			}
		}
	}
	if home != "" {
		fmt.Println("安装目录:", home)
	}
	if listen != "" {
		fmt.Println("监听地址:", listen)
	}
	if hasSystemd() {
		active, _ := exec.Command("systemctl", "is-active", serviceName).Output()
		fmt.Printf("systemd 服务: %s", active)
	}
	pids := findPanelPIDs()
	if len(pids) > 0 {
		fmt.Printf("面板进程: 运行中 (PID %s)\n", strings.Join(intsToStrs(pids), ", "))
		if home != "" {
			if data, err := os.ReadFile(filepath.Join(home, "etc", "port-map.json")); err == nil {
				plugins := 0
				for _, line := range strings.Split(string(data), "\n") {
					if strings.Contains(line, `"port"`) {
						plugins++
					}
				}
				fmt.Printf("运行中插件: %d 个\n", plugins)
			}
		}
	} else {
		fmt.Println("面板进程: 未运行")
	}
}

func cliLog(args []string) {
	n := 100
	if len(args) > 1 && args[1] == "-n" && len(args) > 2 {
		if v, err := strconv.Atoi(args[2]); err == nil && v > 0 {
			n = v
		}
	}
	// 确定安装目录：运行中进程环境 > 自身环境变量 > 二进制位置推导
	home := resolveHome()
	if home == "" {
		fmt.Println("无法确定安装目录，请设置 PANEL_HOME")
		os.Exit(1)
	}
	logPath := filepath.Join(home, "logs", "panel.log")
	data, err := os.ReadFile(logPath)
	if err != nil {
		fmt.Println("日志文件不存在:", logPath)
		os.Exit(1)
	}
	lines := strings.Split(strings.TrimRight(string(data), "\n"), "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	fmt.Println(strings.Join(lines, "\n"))
}

func intsToStrs(in []int) []string {
	out := make([]string, len(in))
	for i, v := range in {
		out[i] = strconv.Itoa(v)
	}
	return out
}
