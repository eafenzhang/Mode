//go:build !windows
// +build !windows

package main

import (
	"fmt"
	"os"
)

func cmdStart(args []string) int {
	fmt.Fprintln(os.Stderr, "start is only supported on Windows")
	return 2
}

func cmdStop(args []string) int {
	fmt.Fprintln(os.Stderr, "stop is only supported on Windows")
	return 2
}
