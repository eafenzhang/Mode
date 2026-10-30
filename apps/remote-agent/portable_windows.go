//go:build windows
// +build windows

package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// cmdLaunch 是免安装 exe 的默认入口：配置优先放在 exe 同目录；不可写时回退到用户目录。
// 之后由守护进程拉起并看护 agent 子进程。
func cmdLaunch() int {
	configPath := resolveConfigPath("")
	var cfg *Config
	pairOnStart := false
	if _, statErr := os.Stat(configPath); statErr != nil {
		if !os.IsNotExist(statErr) {
			showErrorDialog("Mode Go", "无法读取配置：\n"+statErr.Error())
			return 1
		}
		autostart, tray, ok := promptInstallOptions(
			"首次运行 Mode Go",
			"将以免安装方式在下面位置保存配置：\n"+configPath+
				"\n\n启动后会弹出配对页，可输入 Mode 桌面端生成的 6 位配对码。",
			"开始使用",
			true,
			true,
		)
		if !ok {
			return 0
		}
		created, err := createPortableConfig(configPath, autostart, tray)
		if err != nil {
			showErrorDialog("Mode Go", "创建配置失败：\n"+err.Error())
			return 1
		}
		cfg = created
		pairOnStart = true
		exe, _ := os.Executable()
		if cfg.Panel.Autostart {
			_ = installAutostartWithExe(exe, configPath)
		}
		// 未勾选时不删除已有自启项：它可能属于另一个已安装实例；关闭应通过托盘开关显式进行。
	} else {
		loaded, err := loadConfig(configPath)
		if err != nil {
			showErrorDialog("Mode Go", "配置无法读取：\n"+err.Error())
			return 1
		}
		if !loaded.panelConfigured {
			// 旧安装/旧配置升级：补上 UI 默认值；是否开机自启沿用现有注册表状态。
			loaded.Panel = PanelConfig{
				Enabled:   true,
				ShowTray:  true,
				Autostart: autostartEntryExists(),
			}
			loaded.panelConfigured = true
			if err := saveConfig(loaded, configPath); err != nil {
				showErrorDialog("Mode Go", "保存配置失败：\n"+err.Error())
				return 1
			}
			// 旧自启动命令直接跑 serve；迁移到带 supervisor 的 start。
			if loaded.Panel.Autostart {
				if exe, err := os.Executable(); err == nil {
					_ = installAutostartWithExe(exe, configPath)
				}
			}
		}
		if !loaded.Panel.Enabled {
			loaded.Panel.Enabled = true
			if err := saveConfig(loaded, configPath); err != nil {
				showErrorDialog("Mode Go", "保存配置失败：\n"+err.Error())
				return 1
			}
		}
		cfg = loaded
	}
	return runSupervisor(cfg, true, pairOnStart)
}

func createPortableConfig(configPath string, autostart, tray bool) (*Config, error) {
	cfg := defaultConfig()
	cfg.Bind = "0.0.0.0"
	cfg.Port = defaultPort
	agentID, err := randomHex(16)
	if err != nil {
		return nil, err
	}
	token, err := newToken()
	if err != nil {
		return nil, err
	}
	cfg.AgentID = agentID
	cfg.Token = token
	cfg.DataDir = filepath.Dir(configPath)
	cfg.AllowShell = true
	cfg.AllowDesktopControl = true
	cfg.AllowDB = true
	cfg.Panel = PanelConfig{
		Enabled:   true,
		ShowTray:  tray,
		Autostart: autostart,
	}
	cfg.panelConfigured = true
	cfg.configPath = configPath
	if err := saveConfig(cfg, configPath); err != nil {
		return nil, err
	}
	return cfg, nil
}

func autostartEntryExists() bool {
	output, err := exec.Command("reg.exe", "query", runKeyPath, "/v", "ModeAgent").CombinedOutput()
	if err != nil {
		return false
	}
	return strings.Contains(strings.ToLower(decodeConsoleBytes(output, "ansi")), "modeagent")
}
