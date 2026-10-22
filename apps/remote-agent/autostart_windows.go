//go:build windows
// +build windows

package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"strings"
)

const runKeyPath = `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`

func cmdInstallAutostart(args []string) int {
	fs := flag.NewFlagSet("install-autostart", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	fs.Bool("minimized", true, "ignored; kept for CLI compatibility")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	configPath := resolveConfigPath(*configFlag)
	if _, err := os.Stat(configPath); err != nil {
		fmt.Fprintf(os.Stderr, "config not found: %s\n", configPath)
		return 1
	}
	if err := installAutostart(configPath); err != nil {
		fmt.Fprintf(os.Stderr, "install autostart: %v\n", err)
		return 1
	}
	fmt.Printf("autostart=installed\nconfig=%s\n", configPath)
	return 0
}

func cmdUninstallAutostart(args []string) int {
	fs := flag.NewFlagSet("uninstall-autostart", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if err := uninstallAutostart(); err != nil {
		fmt.Fprintf(os.Stderr, "uninstall autostart: %v\n", err)
		return 1
	}
	fmt.Println("autostart=removed")
	return 0
}

func installAutostart(configPath string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	return installAutostartWithExe(exe, configPath)
}

// 自启动直接指向 GUI 子系统的 exe；不再经过 cmd.exe，因此登录时不会闪命令行窗口。
func installAutostartWithExe(exe, configPath string) error {
	command := fmt.Sprintf(`"%s" start --config "%s"`, exe, configPath)
	output, err := exec.Command("reg.exe", "add", runKeyPath, "/v", "ModeAgent", "/t", "REG_SZ", "/d", command, "/f").CombinedOutput()
	if err != nil {
		return fmt.Errorf("%v: %s", err, strings.TrimSpace(decodeConsoleBytes(output, "ansi")))
	}
	return nil
}

func uninstallAutostart() error {
	output, err := exec.Command("reg.exe", "delete", runKeyPath, "/v", "ModeAgent", "/f").CombinedOutput()
	if err != nil {
		// reg.exe 在中文系统上按 ANSI/OEM 输出；直接转 string 会让“找不到”永远匹配失败，
		// 卸载时把“本来就没有自启动项”误报成失败。
		text := strings.ToLower(decodeConsoleBytes(output, "ansi"))
		if strings.Contains(text, "unable to find") || strings.Contains(text, "找不到") {
			return nil
		}
		return fmt.Errorf("%v: %s", err, strings.TrimSpace(decodeConsoleBytes(output, "ansi")))
	}
	return nil
}
