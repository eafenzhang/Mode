package main

import (
	"io/ioutil"
	"os"
	"path/filepath"
	"time"
)

// stopFile 是“用户明确退出”与“崩溃/被杀”之间的唯一区分：
// 托盘退出、/v1/shutdown 与 stop 命令写这个文件；守护进程看到后不再重启 agent。
const stopFileName = "mode-agent.stop"

func supervisorStopPath(cfg *Config) string {
	if cfg == nil {
		return ""
	}
	dir := cfg.DataDir
	if dir == "" && cfg.configPath != "" {
		dir = filepath.Dir(cfg.configPath)
	}
	if dir == "" {
		return ""
	}
	return filepath.Join(dir, stopFileName)
}

func writeStopFile(cfg *Config) {
	path := supervisorStopPath(cfg)
	if path == "" {
		return
	}
	_ = ioutil.WriteFile(path, []byte(time.Now().UTC().Format(time.RFC3339Nano)), 0600)
}

func removeStopFile(cfg *Config) {
	path := supervisorStopPath(cfg)
	if path == "" {
		return
	}
	// 删除失败不阻塞退出；下次启动会覆盖/清理。
	_ = os.Remove(path)
}
