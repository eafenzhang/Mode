//go:build !windows
// +build !windows

package main

import "time"

func discoverDesktops(timeout time.Duration) []desktopPeer {
	return nil
}
