//go:build windows
// +build windows

package main

import (
	"flag"
	"fmt"
	"hash/fnv"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"
	"unsafe"
)

var procCreateMutexW = kernel32.NewProc("CreateMutexW")

// cmdStart 是免安装形态与自启动的入口：当前进程就是守护进程，负责拉起并看护 agent 子进程。
func cmdStart(args []string) int {
	fs := flag.NewFlagSet("start", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	openPanel := fs.Bool("open-panel", false, "open the panel once the agent is healthy")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	return runSupervisor(cfg, *openPanel, false)
}

// cmdStop 让守护进程与 agent 一起退出：写 stop 文件后再请求 HTTP 关闭。
func cmdStop(args []string) int {
	fs := flag.NewFlagSet("stop", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	writeStopFile(cfg)
	_ = requestShutdown(cfg)
	return 0
}

func runSupervisor(cfg *Config, openPanel, pairOnStart bool) int {
	if cfg == nil || cfg.configPath == "" {
		return 1
	}
	openPanelRequested := openPanel
	removeStopFile(cfg)
	mutex, acquired := acquireSupervisorMutex(supervisorMutexName(cfg.configPath))
	if !acquired {
		if openPanel {
			openAgentPanelWindow(cfg)
		}
		return 0
	}
	defer closeSupervisorMutex(mutex)

	supervisorLog(cfg, "supervisor started config=%s", cfg.configPath)
	notifiedForeign := false
	lastSpawn := time.Time{}
	failures := 0
	for {
		if stopRequested(cfg) {
			removeStopFile(cfg)
			supervisorLog(cfg, "stop requested; supervisor exiting")
			return 0
		}
		healthy, foreign := supervisorProbe(cfg)
		if healthy {
			if openPanel {
				openAgentPanelWindow(cfg)
				openPanel = false
			}
			if failures > 0 {
				supervisorLog(cfg, "agent healthy again")
			}
			failures = 0
			lastSpawn = time.Time{}
			time.Sleep(2 * time.Second)
			continue
		}
		if foreign {
			if !notifiedForeign {
				supervisorLog(cfg, "port %d is served by another mode-agent (token mismatch)", cfg.Port)
				messageBox(
					"Mode Agent",
					fmt.Sprintf("端口 %d 已被另一个 Mode Agent 占用。\n请先退出旧实例，或修改本实例的端口。", cfg.Port),
					mbOK|mbIconWarning,
				)
				notifiedForeign = true
			}
			time.Sleep(10 * time.Second)
			continue
		}
		if !lastSpawn.IsZero() && time.Since(lastSpawn) < 5*time.Second {
			time.Sleep(500 * time.Millisecond)
			continue
		}
		if _, err := startAgentChild(cfg, openPanelRequested, pairOnStart); err != nil {
			supervisorLog(cfg, "start agent failed: %v", err)
			time.Sleep(5 * time.Second)
			continue
		}
		// 配对页只在首次拉起时出现；后续崩溃重启不再重复弹。
		pairOnStart = false
		lastSpawn = time.Now()
		deadline := time.Now().Add(10 * time.Second)
		for time.Now().Before(deadline) {
			if stopRequested(cfg) {
				removeStopFile(cfg)
				supervisorLog(cfg, "stop requested during agent startup")
				return 0
			}
			if healthy, _ := supervisorProbe(cfg); healthy {
				break
			}
			time.Sleep(500 * time.Millisecond)
		}
		if healthy, _ := supervisorProbe(cfg); !healthy {
			failures++
			delay := time.Duration(failures) * time.Second
			if delay > 30*time.Second {
				delay = 30 * time.Second
			}
			supervisorLog(cfg, "agent did not become healthy; retrying in %s", delay)
			time.Sleep(delay)
		}
	}
}

func startAgentChild(cfg *Config, openPanel, pairOnStart bool) (*exec.Cmd, error) {
	exe, err := os.Executable()
	if err != nil {
		return nil, err
	}
	// 本地面板固定启用；supervisor 总是以 UI 模式拉起 agent，保证托盘/面板恢复入口存在。
	args := []string{"serve", "--config", cfg.configPath, "--ui"}
	if openPanel {
		args = append(args, "--open-panel")
	}
	if pairOnStart {
		args = append(args, "--pair-on-start")
	}
	command := exec.Command(exe, args...)
	command.SysProcAttr = newGuiSysProcAttr()
	var logFile *os.File
	if cfg.DataDir != "" {
		logPath := filepath.Join(cfg.DataDir, "agent.log")
		if info, statErr := os.Stat(logPath); statErr == nil && info.Size() > 2<<20 {
			_ = os.Remove(logPath)
		}
		logFile, _ = os.OpenFile(logPath, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	}
	if logFile != nil {
		command.Stdout = logFile
		command.Stderr = logFile
	}
	if err := command.Start(); err != nil {
		if logFile != nil {
			logFile.Close()
		}
		return nil, err
	}
	go func(cmd *exec.Cmd, file *os.File) {
		err := cmd.Wait()
		if file != nil {
			file.Close()
		}
		supervisorLog(cfg, "agent pid=%d exited: %v", cmd.Process.Pid, err)
	}(command, logFile)
	return command, nil
}

// supervisorProbe 同时验证健康端点与 token：端口被另一个 agent 占用时不能误判为已就绪。
func supervisorProbe(cfg *Config) (bool, bool) {
	if !agentHealthy(cfg) {
		return false, false
	}
	client := &http.Client{Timeout: 700 * time.Millisecond}
	request, err := http.NewRequest("GET", localAgentBaseURL(cfg)+"/v1/info", nil)
	if err != nil {
		return false, false
	}
	request.Header.Set("Authorization", "Bearer "+cfg.Token)
	response, err := client.Do(request)
	if err != nil {
		return false, false
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusOK {
		return true, false
	}
	if response.StatusCode == http.StatusUnauthorized || response.StatusCode == http.StatusForbidden {
		return false, true
	}
	return false, false
}

func supervisorMutexName(configPath string) string {
	hasher := fnv.New32a()
	_, _ = hasher.Write([]byte(strings.ToLower(filepath.Clean(configPath))))
	return fmt.Sprintf("Local\\ModeAgentSupervisor_%08x", hasher.Sum32())
}

func acquireSupervisorMutex(name string) (uintptr, bool) {
	ptr, err := syscall.UTF16PtrFromString(name)
	if err != nil {
		return 0, false
	}
	handle, _, callErr := procCreateMutexW.Call(0, 0, uintptr(unsafe.Pointer(ptr)))
	runtime.KeepAlive(ptr)
	if handle == 0 {
		return 0, false
	}
	if errno, ok := callErr.(syscall.Errno); ok && errno == 183 { // ERROR_ALREADY_EXISTS
		procCloseHandle.Call(handle)
		return 0, false
	}
	return handle, true
}

func closeSupervisorMutex(handle uintptr) {
	if handle != 0 {
		procCloseHandle.Call(handle)
	}
}

func stopRequested(cfg *Config) bool {
	path := supervisorStopPath(cfg)
	if path == "" {
		return false
	}
	_, err := os.Stat(path)
	return err == nil
}

func supervisorLog(cfg *Config, format string, args ...interface{}) {
	if cfg == nil || cfg.DataDir == "" {
		return
	}
	path := filepath.Join(cfg.DataDir, "supervisor.log")
	if info, err := os.Stat(path); err == nil && info.Size() > 1<<20 {
		_ = os.Remove(path)
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return
	}
	fmt.Fprintf(file, "%s %s\n", time.Now().Format(time.RFC3339), fmt.Sprintf(format, args...))
	file.Close()
}
