//go:build windows
// +build windows

package main

import (
	"io/ioutil"
	"os"
	"path/filepath"
	"testing"
)

func TestCreatePortableConfig(t *testing.T) {
	dir, err := ioutil.TempDir("", "mode-agent-portable-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	defer os.RemoveAll(dir)
	path := filepath.Join(dir, "mode-agent.json")
	cfg, err := createPortableConfig(path, true, true)
	if err != nil {
		t.Fatalf("create portable config: %v", err)
	}
	if cfg.configPath != path || cfg.DataDir != dir {
		t.Fatalf("unexpected paths: config=%s data=%s", cfg.configPath, cfg.DataDir)
	}
	if cfg.Bind != "0.0.0.0" || cfg.Port != defaultPort {
		t.Fatalf("unexpected bind: %s:%d", cfg.Bind, cfg.Port)
	}
	if cfg.Token == "" || cfg.AgentID == "" {
		t.Fatalf("token and agent id must be generated")
	}
	if !cfg.Panel.Enabled || !cfg.Panel.ShowTray || !cfg.Panel.Autostart || !cfg.panelConfigured {
		t.Fatalf("unexpected panel defaults: %+v", cfg.Panel)
	}
	loaded, err := loadConfig(path)
	if err != nil {
		t.Fatalf("load portable config: %v", err)
	}
	if !loaded.Panel.Enabled || !loaded.Panel.ShowTray || !loaded.Panel.Autostart {
		t.Fatalf("panel settings did not persist: %+v", loaded.Panel)
	}

	disabledPath := filepath.Join(dir, "disabled.json")
	disabled, err := createPortableConfig(disabledPath, false, false)
	if err != nil {
		t.Fatalf("create disabled config: %v", err)
	}
	if !disabled.Panel.Enabled || disabled.Panel.ShowTray || disabled.Panel.Autostart {
		t.Fatalf("panel must stay enabled; tray/autostart may be off: %+v", disabled.Panel)
	}
}
