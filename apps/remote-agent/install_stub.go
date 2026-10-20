// +build !windows

package main

import (
	"fmt"
	"os"
)

func cmdInstall(args []string) int {
	fmt.Fprintln(os.Stderr, "install is only supported on Windows")
	return 1
}

func cmdSetup(args []string) int {
	fmt.Fprintln(os.Stderr, "setup is only supported on Windows")
	return 1
}

func cmdUninstall(args []string) int {
	fmt.Fprintln(os.Stderr, "uninstall is only supported on Windows")
	return 1
}
