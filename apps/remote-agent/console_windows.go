//go:build windows
// +build windows

package main

import (
	"os"
	"strings"
)

var (
	procAttachConsole = kernel32.NewProc("AttachConsole")
	procGetStdHandle  = kernel32.NewProc("GetStdHandle")
	procGetFileType   = kernel32.NewProc("GetFileType")
)

// maybeAttachConsole 只为需要标准输出的命令附加到父控制台。exe 本身是 GUI 子系统，
// 所以从资源管理器双击不会出现控制台窗口；从 cmd/PowerShell 运行时仍能看到 CLI 输出。
func maybeAttachConsole(args []string) {
	if !wantsConsole(args) {
		return
	}
	// stdout 已被重定向到文件/管道时不能再改成 CONOUT$，否则脚本拿不到输出。
	if stdoutHandleUsable() {
		return
	}
	const attachParentProcess = ^uintptr(0) // ATTACH_PARENT_PROCESS
	ok, _, _ := procAttachConsole.Call(attachParentProcess)
	if ok == 0 {
		return
	}
	if out, err := os.OpenFile("CONOUT$", os.O_WRONLY, 0); err == nil {
		os.Stdout = out
		os.Stderr = out
	}
	if in, err := os.OpenFile("CONIN$", os.O_RDONLY, 0); err == nil {
		os.Stdin = in
	}
}

func stdoutHandleUsable() bool {
	const stdOutputHandle = uintptr(0xFFFFFFF5) // STD_OUTPUT_HANDLE (-11) as DWORD
	handle, _, _ := procGetStdHandle.Call(stdOutputHandle)
	if handle == 0 || handle == ^uintptr(0) {
		return false
	}
	fileType, _, _ := procGetFileType.Call(handle)
	return fileType != 0 // FILE_TYPE_UNKNOWN == 0
}

func wantsConsole(args []string) bool {
	if len(args) < 2 {
		return false
	}
	switch strings.ToLower(strings.TrimSpace(args[1])) {
	case "start", "supervise", "panel", "open":
		return false
	case "serve":
		for _, arg := range args[2:] {
			if arg == "--ui" || arg == "--open-panel" {
				return false
			}
		}
		return true
	default:
		return true
	}
}
