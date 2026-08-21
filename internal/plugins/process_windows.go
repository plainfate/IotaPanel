//go:build windows

package plugins

// processAlive 判断进程是否存活。
// Windows 无 kill(pid,0)，这里始终返回 true：等满优雅退出窗口后由 SIGKILL 兜底。
func processAlive(pid int) bool { return true }
