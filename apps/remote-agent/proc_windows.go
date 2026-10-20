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

func exitCode(state *os.ProcessState) int {
	if state == nil {
		return -1
	}
	if status, ok := state.Sys().(syscall.WaitStatus); ok {
		return status.ExitStatus()
	}
	return -1
}
