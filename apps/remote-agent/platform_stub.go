// +build !windows

package main

import (
	"os"
	"runtime"
)

func platformInfo() (*InfoResult, *APIError) {
	hostname, _ := os.Hostname()
	return &InfoResult{
		Hostname:         hostname,
		User:             os.Getenv("USER"),
		OS:               runtime.GOOS,
		Arch:             runtime.GOARCH,
		DesktopAvailable: false,
		Screen:           ScreenSize{},
	}, nil
}

func captureScreen(req ScreenRequest) (*ScreenResult, *APIError) {
	return nil, apiError("capability_unavailable", "desktop capture is only supported on Windows", 501)
}

func injectMouse(req MouseRequest) (*MouseResult, *APIError) {
	return nil, apiError("capability_unavailable", "mouse injection is only supported on Windows", 501)
}

func injectKeyboard(req KeyboardRequest) (*KeyboardResult, *APIError) {
	return nil, apiError("capability_unavailable", "keyboard injection is only supported on Windows", 501)
}

func listWindows() (*WindowListResult, *APIError) {
	return nil, apiError("capability_unavailable", "window enumeration is only supported on Windows", 501)
}

func focusWindow(req WindowFocusRequest) (*WindowFocusResult, *APIError) {
	return nil, apiError("capability_unavailable", "window focus is only supported on Windows", 501)
}

func listDBDrivers() (*DBDriversResult, *APIError) {
	return nil, apiError("capability_unavailable", "ODBC is only supported on Windows", 501)
}

func queryDB(req DBQueryRequest, cfg *Config) (*DBQueryResult, *APIError) {
	return nil, apiError("capability_unavailable", "ODBC is only supported on Windows", 501)
}

func replaceFile(source, target string) error {
	return os.Rename(source, target)
}

func processList() (*ProcessListResult, *APIError) {
	return nil, apiError("capability_unavailable", "process listing is only supported on Windows", 501)
}

func processKill(req ProcessKillRequest) (*ProcessKillResult, *APIError) {
	return nil, apiError("capability_unavailable", "process termination is only supported on Windows", 501)
}
