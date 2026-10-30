//go:build !go1.13
// +build !go1.13

package main

import (
	"fmt"
	"os"
)

// XP 工具链（Go 1.10）没有 crypto/ed25519，无法完成 GitHub Device Flow；
// 云账号登录需要现代工具链构建的 agent（docs/specs/cloud-personal-accounts.md）。
func cmdCloudLogin(args []string) int {
	fmt.Fprintln(os.Stderr, "此构建（XP 兼容）不支持 Mode Cloud 登录；请使用现代工具链构建的 mode-agent。")
	return 1
}

func runCloudLogin(cfg *Config, configPath string, noDialog bool) cloudLoginResult {
	_ = cfg
	_ = configPath
	_ = noDialog
	return cloudLoginResult{Error: "此构建（XP 兼容）不支持 Mode Cloud 登录"}
}

func refreshCloudEnrollment(cfg *Config) error {
	_ = cfg
	return nil
}

func maintainCloudEnrollment(cfg *Config) {
	_ = cfg
}
