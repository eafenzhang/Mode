//go:build !windows
// +build !windows

package main

import "fmt"

type agentUIHandle struct{}

func startAgentUI(cfg *Config, panel *panelController, requestShutdown func()) (*agentUIHandle, error) {
	return nil, fmt.Errorf("panel UI is only supported on Windows")
}

func (ui *agentUIHandle) stop() {}

func (ui *agentUIHandle) show() {}

func agentPanelVisible() bool {
	return false
}

func panelSupported() bool {
	return false
}

func notifyPanelChanged() {}

func showAgentPanel() bool {
	return false
}

func openAgentPanelWindow(cfg *Config) bool {
	return false
}
