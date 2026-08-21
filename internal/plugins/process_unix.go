//go:build !windows

package plugins

import "syscall"

// processAlive 判断进程是否存活（unix：kill(pid, 0)）。
func processAlive(pid int) bool {
	return syscall.Kill(pid, 0) == nil
}
