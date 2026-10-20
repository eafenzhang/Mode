// +build !windows

package main

import (
	"fmt"
	"os"
)

func cmdServe(args []string) int {
	fmt.Fprintln(os.Stderr, "mode-agent serve is only supported on Windows")
	return 1
}
