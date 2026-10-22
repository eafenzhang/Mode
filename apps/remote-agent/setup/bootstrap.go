//go:build windows
// +build windows

package main

import (
	"archive/zip"
	"encoding/binary"
	"fmt"
	"io"
	"io/ioutil"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"syscall"
	"unsafe"
)

const (
	footerSize  = 24
	footerMagic = "MODEAGNT"
)

var (
	user32            = syscall.NewLazyDLL("user32.dll")
	kernel32          = syscall.NewLazyDLL("kernel32.dll")
	procMessageBoxW   = user32.NewProc("MessageBoxW")
	procAttachConsole = kernel32.NewProc("AttachConsole")
	procGetStdHandle  = kernel32.NewProc("GetStdHandle")
	procGetFileType   = kernel32.NewProc("GetFileType")
)

// 安装包是 GUI 子系统；从 cmd/PowerShell 运行 --quiet 等命令时附加到父控制台，
// 这样脚本仍能拿到输出。资源管理器双击时 AttachConsole 失败，不会创建新窗口。
func attachParentConsole() {
	if stdoutHandleUsable() {
		return
	}
	const attachParentProcess = ^uintptr(0)
	if ok, _, _ := procAttachConsole.Call(attachParentProcess); ok == 0 {
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
	const stdOutputHandle = uintptr(0xFFFFFFF5)
	handle, _, _ := procGetStdHandle.Call(stdOutputHandle)
	if handle == 0 || handle == ^uintptr(0) {
		return false
	}
	fileType, _, _ := procGetFileType.Call(handle)
	return fileType != 0
}

func showError(message string) {
	titlePtr, _ := syscall.UTF16PtrFromString("Mode Agent 安装器")
	textPtr, _ := syscall.UTF16PtrFromString(message)
	procMessageBoxW.Call(
		0,
		uintptr(unsafe.Pointer(textPtr)),
		uintptr(unsafe.Pointer(titlePtr)),
		0x00000010, // MB_ICONERROR
	)
	runtime.KeepAlive(titlePtr)
	runtime.KeepAlive(textPtr)
}

func payloadName() string {
	arch := os.Getenv("PROCESSOR_ARCHITEW6432")
	if arch == "AMD64" || arch == "ARM64" {
		return "mode-agent-x64.exe"
	}
	return "mode-agent-x86.exe"
}

func run() error {
	exePath, err := os.Executable()
	if err != nil {
		return err
	}
	file, err := os.Open(exePath)
	if err != nil {
		return err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return err
	}
	size := info.Size()
	if size <= footerSize {
		return fmt.Errorf("安装包缺少载荷")
	}
	footer := make([]byte, footerSize)
	if _, err := file.ReadAt(footer, size-footerSize); err != nil {
		return err
	}
	if string(footer[:8]) != footerMagic {
		return fmt.Errorf("安装包格式不匹配")
	}
	zipOffset := int64(binary.LittleEndian.Uint64(footer[8:16]))
	zipLength := int64(binary.LittleEndian.Uint64(footer[16:24]))
	if zipOffset <= 0 || zipLength <= 0 || zipOffset+zipLength > size-footerSize {
		return fmt.Errorf("安装包载荷偏移无效")
	}
	archive, err := zip.NewReader(io.NewSectionReader(file, zipOffset, zipLength), zipLength)
	if err != nil {
		return fmt.Errorf("读取安装包载荷失败：%v", err)
	}
	name := payloadName()
	var payload *zip.File
	for _, candidate := range archive.File {
		if candidate.Name == name {
			payload = candidate
			break
		}
	}
	if payload == nil {
		return fmt.Errorf("安装包缺少 %s", name)
	}
	tempDir, err := ioutil.TempDir("", "mode-agent-setup-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tempDir)
	target := filepath.Join(tempDir, "mode-agent.exe")
	reader, err := payload.Open()
	if err != nil {
		return err
	}
	writer, err := os.OpenFile(target, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0700)
	if err != nil {
		reader.Close()
		return err
	}
	_, copyErr := io.Copy(writer, reader)
	reader.Close()
	closeErr := writer.Close()
	if copyErr != nil {
		return copyErr
	}
	if closeErr != nil {
		return closeErr
	}
	args := os.Args[1:]
	if len(args) == 0 {
		// 双击安装包仍进入安装流程；免安装形态由 agent exe 自身承担。
		args = []string{"setup"}
	}
	command := exec.Command(target, args...)
	command.Stdin = os.Stdin
	command.Stdout = os.Stdout
	command.Stderr = os.Stderr
	return command.Run()
}

func main() {
	attachParentConsole()
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		showError(err.Error())
		os.Exit(1)
	}
}
