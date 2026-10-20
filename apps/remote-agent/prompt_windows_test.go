// +build windows

package main

import (
	"syscall"
	"testing"
	"time"
	"unsafe"
)

func TestPromptForText(t *testing.T) {
	const title = "Mode Agent Test Prompt"
	procFindWindowW := user32.NewProc("FindWindowW")
	procFindWindowExW := user32.NewProc("FindWindowExW")
	procSetWindowTextW := user32.NewProc("SetWindowTextW")
	procPostMessageW := user32.NewProc("PostMessageW")
	go func() {
		titlePtr, _ := syscall.UTF16PtrFromString(title)
		var window uintptr
		deadline := time.Now().Add(3 * time.Second)
		for time.Now().Before(deadline) {
			window, _, _ = procFindWindowW.Call(0, uintptr(unsafe.Pointer(titlePtr)))
			if window != 0 {
				break
			}
			time.Sleep(20 * time.Millisecond)
		}
		if window == 0 {
			return
		}
		editClass, _ := syscall.UTF16PtrFromString("Edit")
		edit, _, _ := procFindWindowExW.Call(window, 0, uintptr(unsafe.Pointer(editClass)), 0)
		if edit == 0 {
			procPostMessageW.Call(window, 0x0010, 0, 0) // WM_CLOSE
			return
		}
		textPtr, _ := syscall.UTF16PtrFromString("hello")
		procSetWindowTextW.Call(edit, uintptr(unsafe.Pointer(textPtr)))
		procPostMessageW.Call(window, 0x0111, 1, 0) // WM_COMMAND IDOK
	}()
	result, ok := promptForText(title, "test label", "")
	if !ok || result != "hello" {
		t.Fatalf("expected hello, got ok=%v result=%q", ok, result)
	}
}
