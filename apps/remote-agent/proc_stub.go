// +build !windows

package main

import (
	"os"
	"syscall"
)

func newSysProcAttr() *syscall.SysProcAttr {
	return nil
}

func exitCode(state *os.ProcessState) int {
	if state == nil {
		return -1
	}
	if status, ok := state.Sys().(syscall.WaitStatus); ok {
		if status.Signaled() {
			return 128 + int(status.Signal())
		}
		return status.ExitStatus()
	}
	return -1
}
