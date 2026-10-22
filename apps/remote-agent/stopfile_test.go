package main

import (
	"io/ioutil"
	"os"
	"path/filepath"
	"testing"
)

func TestStopFileLifecycle(t *testing.T) {
	dir, err := ioutil.TempDir("", "mode-agent-stop-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	defer os.RemoveAll(dir)
	cfg := &Config{DataDir: dir, configPath: filepath.Join(dir, "mode-agent.json")}
	path := supervisorStopPath(cfg)
	if path != filepath.Join(dir, stopFileName) {
		t.Fatalf("unexpected stop path: %s", path)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("stop file should not exist yet")
	}
	writeStopFile(cfg)
	if _, err := os.Stat(path); err != nil {
		t.Fatalf("stop file was not written: %v", err)
	}
	removeStopFile(cfg)
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("stop file was not removed")
	}
}
