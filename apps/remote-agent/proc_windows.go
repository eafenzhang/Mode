// +build windows

package main

import (
	"os"
	"syscall"
)

func newSysProcAttr() *syscall.SysProcAttr {
	return &syscall.SysProcAttr{
		HideWindow:    true,
		CreationFlags: 0x08000000, // CREATE_NO_WINDOW
	}
}

// newGuiSysProcAttr 用于启动 GUI 子系统的 agent 子进程。
// 不能带 HideWindow：它会让 STARTUPINFO 的 SW_HIDE 成为窗口第一次 ShowWindow 的初始状态，
// 导致 serve --open-panel 首次显示被吞掉。
func newGuiSysProcAttr() *syscall.SysProcAttr {
	return &syscall.SysProcAttr{
		CreationFlags: 0x08000000, // CREATE_NO_WINDOW
	}
}

func exitCode(state *os.ProcessState) int {
	if state == nil {
		return -1
	}
	if status, ok := state.Sys().(syscall.WaitStatus); ok {
		return status.ExitStatus()
	}
	return -1
}
