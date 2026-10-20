// +build !windows

package main

import (
	"fmt"
	"os"
)

func cmdInstallAutostart(args []string) int {
	fmt.Fprintln(os.Stderr, "install-autostart is only supported on Windows")
	return 1
}

func cmdUninstallAutostart(args []string) int {
	fmt.Fprintln(os.Stderr, "uninstall-autostart is only supported on Windows")
	return 1
}
