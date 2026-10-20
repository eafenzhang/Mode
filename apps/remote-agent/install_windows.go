// +build windows

package main

import (
	"flag"
	"fmt"
	"io"
	"io/ioutil"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

type installOptions struct {
	Dir         string
	Bind        string
	Port        int
	Token       string
	Force       bool
	NoStart     bool
	NoAutostart bool
	NoFirewall  bool
	Quiet       bool
	NoDesktop   bool
	AllowWrite  bool
	AllowDelete bool
	Yes         bool
	Purge       bool
}

type installPaths struct {
	Dir    string
	Exe    string
	Config string
	Info   string
}

type installInfo struct {
	InstalledAt string
	Exe         string
	Config      string
	InfoPath    string
	Token       string
	AgentID     string
	Name        string
	Host        string
	Port        int
	Addresses   string
	PairCode    string
	PairExpires string
	PairURI     string
	MCPURL      string
	Autostart   string
	Firewall    string
	Agent       string
	PairError   string
	Enrollment  string
	EnrollError string
}

func defaultInstallDir() string {
	if local := strings.TrimSpace(os.Getenv("LOCALAPPDATA")); local != "" {
		return filepath.Join(local, "ModeAgent")
	}
	if roaming := strings.TrimSpace(os.Getenv("APPDATA")); roaming != "" {
		return filepath.Join(roaming, "ModeAgent")
	}
	if profile := strings.TrimSpace(os.Getenv("USERPROFILE")); profile != "" {
		return filepath.Join(profile, "ModeAgent")
	}
	return filepath.Join(os.TempDir(), "ModeAgent")
}

func resolveInstallPaths(dir string) installPaths {
	if strings.TrimSpace(dir) == "" {
		// 从“已安装目录里的 mode-agent.exe”双击时，配置就在 exe 旁边；
		// 不能退回默认目录，否则自定义 --dir 安装会被误判成未安装。
		if exe, err := os.Executable(); err == nil {
			exeDir := filepath.Dir(exe)
			if _, statErr := os.Stat(filepath.Join(exeDir, "mode-agent.json")); statErr == nil {
				dir = exeDir
			}
		}
	}
	if strings.TrimSpace(dir) == "" {
		dir = defaultInstallDir()
	}
	if abs, err := filepath.Abs(dir); err == nil {
		dir = abs
	}
	return installPaths{
		Dir:    dir,
		Exe:    filepath.Join(dir, "mode-agent.exe"),
		Config: filepath.Join(dir, "mode-agent.json"),
		Info:   filepath.Join(dir, "install-info.txt"),
	}
}

func addInstallFlags(fs *flag.FlagSet, opts *installOptions) {
	fs.StringVar(&opts.Dir, "dir", "", "install directory")
	fs.StringVar(&opts.Bind, "bind", "0.0.0.0", "listen address")
	fs.IntVar(&opts.Port, "port", defaultPort, "listen port")
	fs.StringVar(&opts.Token, "token", "", "use a specific bearer token")
	fs.BoolVar(&opts.Force, "force", false, "overwrite an existing config and rotate the token")
	fs.BoolVar(&opts.NoStart, "no-start", false, "install without starting the agent")
	fs.BoolVar(&opts.NoAutostart, "no-autostart", false, "do not add the logon autostart entry")
	fs.BoolVar(&opts.NoFirewall, "no-firewall", false, "do not add a Windows Firewall rule")
	fs.BoolVar(&opts.Quiet, "quiet", false, "print install information instead of a dialog")
	fs.BoolVar(&opts.NoDesktop, "no-desktop", false, "disable remote desktop input")
	fs.BoolVar(&opts.AllowWrite, "allow-write", false, "enable remote file writes")
	fs.BoolVar(&opts.AllowDelete, "allow-delete", false, "enable remote file deletes")
}

func cmdInstall(args []string) int {
	fs := flag.NewFlagSet("install", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	opts := installOptions{}
	addInstallFlags(fs, &opts)
	if err := fs.Parse(args); err != nil {
		return 2
	}
	return runInstall(opts)
}

func cmdSetup(args []string) int {
	fs := flag.NewFlagSet("setup", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	opts := installOptions{}
	addInstallFlags(fs, &opts)
	fs.BoolVar(&opts.Yes, "yes", false, "install without a confirmation dialog")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	paths := resolveInstallPaths(opts.Dir)
	if _, err := os.Stat(paths.Config); err == nil {
		exe, _ := os.Executable()
		runningInstalled := exe != "" && samePath(exe, paths.Exe)
		// 从下载目录再次运行安装包 = 更新；从安装目录的 exe 双击 = 输入桌面配对码。
		if opts.Yes || (!runningInstalled && messageBox("更新 Mode Agent", "检测到已安装。使用当前安装包更新并重新配对？", mbYesNo|mbIconQuestion) == mbYes) {
			return runInstall(opts)
		}
		cfg, loadErr := loadConfig(paths.Config)
		if loadErr != nil {
			messageBox("Mode Agent", "已安装，但配置无法读取：\n"+loadErr.Error(), mbOK|mbIconWarning)
			return 1
		}
		if !agentHealthy(cfg) {
			if _, startErr := startInstalledAgent(paths.Exe, paths.Config, cfg); startErr != nil {
				showErrorDialog("Mode Agent 启动失败", startErr.Error())
				return 1
			}
		}
		return promptAndEnroll(paths, cfg)
	}
	if opts.Yes {
		return runInstall(opts)
	}
	answer := messageBox(
		"安装 Mode Agent",
		"安装后，Mode 可以远程执行命令、读取文件并控制此电脑桌面。\n\n"+
			"安装器会复制程序到当前用户目录、写入登录自启动，并尝试放行 TCP 端口。\n\n继续安装？",
		mbYesNo|mbIconQuestion,
	)
	if answer != mbYes {
		return 0
	}
	return runInstall(opts)
}

func cmdUninstall(args []string) int {
	fs := flag.NewFlagSet("uninstall", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	opts := installOptions{}
	fs.StringVar(&opts.Dir, "dir", "", "install directory")
	fs.BoolVar(&opts.Yes, "yes", false, "uninstall without a confirmation dialog")
	fs.BoolVar(&opts.Purge, "purge", false, "remove config, logs and the installed executable")
	fs.BoolVar(&opts.NoFirewall, "no-firewall", false, "keep the Windows Firewall rule")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	paths := resolveInstallPaths(opts.Dir)
	if !opts.Yes {
		if messageBox("卸载 Mode Agent", "停止被控端、移除登录自启动和防火墙规则？", mbYesNo|mbIconQuestion) != mbYes {
			return 0
		}
	}
	port := defaultPort
	cfg, cfgErr := loadConfig(paths.Config)
	if cfgErr == nil {
		port = cfg.Port
	}
	autostart := "removed"
	if err := uninstallAutostart(); err != nil {
		autostart = "failed: " + err.Error()
	}
	stopInstalledAgent(paths, cfg)
	firewall := "skipped"
	if !opts.NoFirewall {
		firewall = removeFirewallRule(port)
	}
	if opts.Purge {
		removeFile(paths.Config)
		removeFile(paths.Info)
		removeFile(filepath.Join(paths.Dir, "agent-cert.pem"))
		removeFile(filepath.Join(paths.Dir, "agent-key.pem"))
		removeFile(filepath.Join(paths.Dir, "audit.log"))
		removeFile(filepath.Join(paths.Dir, "audit.log.1"))
		removeFile(filepath.Join(paths.Dir, pidFileName))
		if exe, err := os.Executable(); err == nil && !samePath(exe, paths.Exe) {
			removeFileWithRetry(paths.Exe, 3*time.Second)
		}
		removeDirWithRetry(paths.Dir, 2*time.Second)
	}
	fmt.Printf("uninstalled=true\nautostart=%s\nfirewall=%s\npurge=%v\n", autostart, firewall, opts.Purge)
	return 0
}

func runInstall(opts installOptions) int {
	paths := resolveInstallPaths(opts.Dir)
	if err := os.MkdirAll(paths.Dir, 0700); err != nil {
		fmt.Fprintf(os.Stderr, "create install dir: %v\n", err)
		return 1
	}
	existingCfg, _ := loadConfig(paths.Config)
	stopInstalledAgent(paths, existingCfg)
	if err := copySelf(paths.Exe); err != nil {
		fmt.Fprintf(os.Stderr, "copy executable: %v\n", err)
		return 1
	}
	cfg, err := loadOrCreateInstallConfig(opts, paths)
	if err != nil {
		fmt.Fprintf(os.Stderr, "prepare config: %v\n", err)
		return 1
	}
	autostart := "skipped"
	if !opts.NoAutostart {
		if err := installAutostartWithExe(paths.Exe, paths.Config, true); err != nil {
			autostart = "failed: " + err.Error()
		} else {
			autostart = "installed"
		}
	}
	firewall := "skipped"
	if !opts.NoFirewall {
		firewall, _ = ensureFirewallRule(cfg.Port)
	}
	agent := "skipped"
	var startErr error
	if !opts.NoStart {
		agent, startErr = startInstalledAgent(paths.Exe, paths.Config, cfg)
	}
	info := buildInstallInfo(paths, cfg, agent, autostart, firewall, nil, nil)
	info.Enrollment = "pending"
	if opts.NoStart {
		info.Enrollment = "agent_not_started"
	}
	if startErr != nil {
		info.Enrollment = "agent_failed"
	}
	if writeErr := writeInstallInfo(paths.Info, info); writeErr != nil {
		fmt.Fprintf(os.Stderr, "write install info: %v\n", writeErr)
	}
	if opts.Quiet {
		printInstallInfo(info)
		if startErr != nil {
			fmt.Fprintf(os.Stderr, "start agent: %v\n", startErr)
			return 1
		}
		return 0
	}
	if startErr != nil {
		showErrorDialog("Mode Agent 启动失败", startErr.Error())
		return 1
	}
	if opts.NoStart {
		showInfoDialog("Mode Agent 已安装", "已落盘但未启动。\n启动 agent 后运行 mode-agent.exe enroll 完成配对。")
		return 0
	}
	return promptAndEnroll(paths, cfg)
}

func promptAndEnroll(paths installPaths, cfg *Config) int {
	raw, ok := promptForText(
		"Mode Agent 配对",
		"请粘贴 Mode 桌面端生成的配对串（mode-agent://enroll?...），\n或输入 桌面地址:端口 配对码。",
		"",
	)
	if !ok || strings.TrimSpace(raw) == "" {
		appendInstallInfo(paths.Info, "enrollment=canceled")
		showInfoDialog("Mode Agent 已安装", "已安装并启动。\n稍后可运行 mode-agent.exe enroll 完成配对。")
		return 0
	}
	serverName, err := enrollWithDesktop(cfg, raw, "")
	if err != nil {
		appendInstallInfo(paths.Info, "enrollment=failed", "enrollError="+err.Error())
		showErrorDialog("Mode Agent 配对失败", err.Error())
		return 1
	}
	appendInstallInfo(paths.Info, "enrollment=success", "mcpServer="+serverName)
	showInfoDialog("Mode Agent 配对成功", "已登记到 Mode 桌面端。\n请回到远程控制页点击「刷新列表」。")
	return 0
}

func appendInstallInfo(path string, lines ...string) {
	file, err := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return
	}
	for _, line := range lines {
		fmt.Fprintln(file, line)
	}
	file.Close()
}

func loadOrCreateInstallConfig(opts installOptions, paths installPaths) (*Config, error) {
	if _, err := os.Stat(paths.Config); err == nil && !opts.Force {
		return loadConfig(paths.Config)
	}
	cfg := defaultConfig()
	cfg.Bind = opts.Bind
	if strings.TrimSpace(cfg.Bind) == "" {
		cfg.Bind = "0.0.0.0"
	}
	cfg.Port = opts.Port
	if cfg.Port <= 0 {
		cfg.Port = defaultPort
	}
	agentID, err := randomHex(16)
	if err != nil {
		return nil, err
	}
	cfg.AgentID = agentID
	token := strings.TrimSpace(opts.Token)
	if token == "" {
		token, err = newToken()
		if err != nil {
			return nil, err
		}
	}
	cfg.Token = token
	cfg.DataDir = paths.Dir
	cfg.AllowShell = true
	cfg.AllowDesktopControl = !opts.NoDesktop
	cfg.AllowDB = true
	cfg.AllowWrite = opts.AllowWrite
	cfg.AllowDelete = opts.AllowDelete
	cfg.AllowDBWrite = false
	cfg.DSNs = map[string]string{}
	if err := saveConfig(cfg, paths.Config); err != nil {
		return nil, err
	}
	return cfg, nil
}

func copySelf(target string) error {
	source, err := os.Executable()
	if err != nil {
		return err
	}
	if samePath(source, target) {
		return nil
	}
	tmp := target + ".new"
	if err := copyFile(source, tmp); err != nil {
		return err
	}
	if err := replaceFile(tmp, target); err != nil {
		os.Remove(tmp)
		return err
	}
	return nil
}

func copyFile(source, target string) error {
	in, err := os.Open(source)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(target, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0700)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	return out.Close()
}

func stopInstalledAgent(paths installPaths, cfg *Config) {
	dataDir := paths.Dir
	if cfg != nil {
		if strings.TrimSpace(cfg.DataDir) != "" {
			dataDir = cfg.DataDir
		}
		_ = requestShutdown(cfg)
		deadline := time.Now().Add(3 * time.Second)
		for time.Now().Before(deadline) {
			if !agentHealthy(cfg) {
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
	}
	pid, err := readPIDFile(dataDir)
	if err != nil {
		return
	}
	command := exec.Command("taskkill.exe", "/F", "/PID", strconv.Itoa(pid))
	command.SysProcAttr = newSysProcAttr()
	_, _ = command.CombinedOutput()
	removePIDFile(dataDir)
	time.Sleep(500 * time.Millisecond)
}

func requestShutdown(cfg *Config) error {
	client := &http.Client{Timeout: 2 * time.Second}
	request, err := http.NewRequest("POST", localAgentBaseURL(cfg)+"/v1/shutdown", nil)
	if err != nil {
		return err
	}
	request.Header.Set("Authorization", "Bearer "+cfg.Token)
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	response.Body.Close()
	return nil
}

func startInstalledAgent(exe, configPath string, cfg *Config) (string, error) {
	if agentHealthy(cfg) {
		return "already-running", nil
	}
	command := exec.Command(exe, "serve", "--config", configPath)
	command.SysProcAttr = newSysProcAttr()
	if err := command.Start(); err != nil {
		return "failed", err
	}
	_ = command.Process.Release()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if agentHealthy(cfg) {
			return "running", nil
		}
		time.Sleep(100 * time.Millisecond)
	}
	return "failed", fmt.Errorf("agent did not become healthy on port %d", cfg.Port)
}

func agentHealthy(cfg *Config) bool {
	client := &http.Client{Timeout: 700 * time.Millisecond}
	response, err := client.Get(localAgentBaseURL(cfg) + "/health")
	if err != nil {
		return false
	}
	defer response.Body.Close()
	return response.StatusCode == http.StatusOK
}

func ensureFirewallRule(port int) (string, error) {
	major, _, _ := windowsVersionNumbers()
	if major <= 5 {
		if output, err := runNetsh("firewall", "show", "portopening"); err == nil && strings.Contains(output, strconv.Itoa(port)) {
			return "already-present", nil
		}
		output, err := runNetsh("firewall", "add", "portopening", "TCP", strconv.Itoa(port), "ModeAgent")
		if err != nil {
			return "failed: " + strings.TrimSpace(output), err
		}
		return "added", nil
	}
	if output, err := runNetsh("advfirewall", "firewall", "show", "rule", "name=ModeAgent"); err == nil && strings.Contains(output, "ModeAgent") {
		return "already-present", nil
	}
	output, err := runNetsh("advfirewall", "firewall", "add", "rule", "name=ModeAgent", "dir=in", "action=allow", "protocol=TCP", "localport="+strconv.Itoa(port))
	if err != nil {
		return "failed: " + strings.TrimSpace(output), err
	}
	return "added", nil
}

func removeFirewallRule(port int) string {
	major, _, _ := windowsVersionNumbers()
	var err error
	if major <= 5 {
		_, err = runNetsh("firewall", "delete", "portopening", "TCP", strconv.Itoa(port))
	} else {
		_, err = runNetsh("advfirewall", "firewall", "delete", "rule", "name=ModeAgent")
	}
	if err != nil {
		return "failed: " + err.Error()
	}
	return "removed"
}

func runNetsh(args ...string) (string, error) {
	command := exec.Command("netsh.exe", args...)
	command.SysProcAttr = newSysProcAttr()
	output, err := command.CombinedOutput()
	return string(output), err
}

func buildInstallInfo(paths installPaths, cfg *Config, agent, autostart, firewall string, code *PairCodeResult, codeErr error) installInfo {
	host := primaryPairAddress()
	info := installInfo{
		InstalledAt: time.Now().Format(time.RFC3339),
		Exe:         paths.Exe,
		Config:      paths.Config,
		InfoPath:    paths.Info,
		Token:       cfg.Token,
		AgentID:     cfg.AgentID,
		Name:        hostnameOr("Mode Agent"),
		Host:        host,
		Port:        cfg.Port,
		Addresses:   strings.Join(localIPv4Addresses(), ","),
		MCPURL:      fmt.Sprintf("http://%s:%d/mcp", host, cfg.Port),
		Autostart:   autostart,
		Firewall:    firewall,
		Agent:       agent,
	}
	if code != nil {
		info.PairCode = code.Code
		info.PairExpires = time.Unix(0, code.ExpiresAt*int64(time.Millisecond)).Format(time.RFC3339)
		info.PairURI = buildPairURI(host, code.Port, code.Code)
	}
	if codeErr != nil {
		info.PairError = codeErr.Error()
	}
	return info
}

func writeInstallInfo(path string, info installInfo) error {
	lines := []string{
		"installedAt=" + info.InstalledAt,
		"exe=" + info.Exe,
		"config=" + info.Config,
		"token=" + info.Token,
		"agentId=" + info.AgentID,
		"name=" + info.Name,
		"host=" + info.Host,
		"port=" + strconv.Itoa(info.Port),
		"addresses=" + info.Addresses,
		"pairCode=" + info.PairCode,
		"pairCodeExpiresAt=" + info.PairExpires,
		"pairUri=" + info.PairURI,
		"mcpUrl=" + info.MCPURL,
		"autostart=" + info.Autostart,
		"firewall=" + info.Firewall,
		"agent=" + info.Agent,
		"pairError=" + info.PairError,
	}
	return ioutil.WriteFile(path, []byte(strings.Join(lines, "\n")+"\n"), 0600)
}

func printInstallInfo(info installInfo) {
	for _, line := range []string{
		"installed=true",
		"exe=" + info.Exe,
		"config=" + info.Config,
		"token=" + info.Token,
		"agentId=" + info.AgentID,
		"host=" + info.Host,
		"port=" + strconv.Itoa(info.Port),
		"pairCode=" + info.PairCode,
		"pairCodeExpiresAt=" + info.PairExpires,
		"pairUri=" + info.PairURI,
		"mcpUrl=" + info.MCPURL,
		"autostart=" + info.Autostart,
		"firewall=" + info.Firewall,
		"agent=" + info.Agent,
		"info=" + info.InfoPath,
		"pairError=" + info.PairError,
		"enrollment=" + info.Enrollment,
		"enrollError=" + info.EnrollError,
	} {
		fmt.Println(line)
	}
}

func showInstallResult(info installInfo) {
	if info.PairCode == "" {
		messageBox("Mode Agent 安装完成",
			"Mode Agent 已安装，但未能生成配对码：\n"+info.PairError+"\n\n详细信息："+info.InfoPath,
			mbOK|mbIconWarning)
		return
	}
	text := fmt.Sprintf(
		"Mode Agent 已安装并启动。\n\n"+
			"在 Mode 桌面端打开「远程控制」→「添加被控端」：\n"+
			"地址：%s\n端口：%d\n配对码：%s\n\n"+
			"也可以直接粘贴：\n%s\n\n"+
			"配对码 10 分钟内有效。\n自启动：%s\n防火墙：%s\n\n详细信息：%s",
		info.Addresses, info.Port, info.PairCode, info.PairURI, info.Autostart, info.Firewall, info.InfoPath,
	)
	messageBox("Mode Agent 安装完成", text, mbOK|mbIconInformation)
}

func samePath(left, right string) bool {
	leftAbs, leftErr := filepath.Abs(left)
	rightAbs, rightErr := filepath.Abs(right)
	if leftErr != nil || rightErr != nil {
		return strings.EqualFold(filepath.Clean(left), filepath.Clean(right))
	}
	return strings.EqualFold(filepath.Clean(leftAbs), filepath.Clean(rightAbs))
}

func removeFile(path string) {
	if strings.TrimSpace(path) != "" {
		os.Remove(path)
	}
}

// 卸载时 agent 可能刚收到 shutdown、进程尚未完全退出，Windows 会锁住安装副本。
// 这里对 exe/目录做短重试，避免“卸载成功但留下 exe”。
func removeFileWithRetry(path string, timeout time.Duration) {
	deadline := time.Now().Add(timeout)
	for {
		err := os.Remove(path)
		if err == nil || os.IsNotExist(err) {
			return
		}
		if time.Now().After(deadline) {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
}

func removeDirWithRetry(path string, timeout time.Duration) {
	deadline := time.Now().Add(timeout)
	for {
		err := os.Remove(path)
		if err == nil || os.IsNotExist(err) {
			return
		}
		if time.Now().After(deadline) {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
}
