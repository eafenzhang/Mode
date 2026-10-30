package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io/ioutil"
	"net"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

const remoteAgentEnrollPath = "/api/remote-agent/enroll"

type enrollTarget struct {
	Host string
	Port int
	Code string
}

type enrollmentPayload struct {
	Code      string   `json:"code"`
	AgentID   string   `json:"agentId"`
	Name      string   `json:"name"`
	Host      string   `json:"host"`
	Port      int      `json:"port"`
	Token     string   `json:"token"`
	Version   string   `json:"version"`
	Platform  string   `json:"platform"`
	Addresses []string `json:"addresses,omitempty"`
}

type enrollmentReply struct {
	OK         bool   `json:"ok"`
	Error      string `json:"error"`
	ServerName string `json:"serverName"`
}

func cmdEnroll(args []string) int {
	fs := flag.NewFlagSet("enroll", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	uri := fs.String("uri", "", "desktop pairing URI")
	desktop := fs.String("desktop", "", "desktop host:port")
	code := fs.String("code", "", "desktop pairing code")
	agentHost := fs.String("agent-host", "", "address the desktop should use to reach this agent")
	noDialog := fs.Bool("no-dialog", false, "do not show dialogs")
	jsonOutput := fs.Bool("json", false, "print JSON")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	raw := strings.TrimSpace(*uri)
	if raw == "" && strings.TrimSpace(*desktop) != "" && strings.TrimSpace(*code) != "" {
		raw = strings.TrimSpace(*desktop) + " " + strings.TrimSpace(*code)
	}
	if raw == "" && strings.TrimSpace(*code) != "" {
		raw = strings.TrimSpace(*code)
	}
	if raw == "" {
		if *noDialog {
			fmt.Fprintln(os.Stderr, "需要 --code，或 --uri / --desktop + --code")
			return 2
		}
		raw, _ = promptForText(
			"Mode Go 配对",
			"请输入 Mode 桌面端生成的 6 位配对码。\n（无需输入链接或地址）",
			"",
		)
	}
	if strings.TrimSpace(raw) == "" {
		fmt.Fprintln(os.Stderr, "未输入配对信息")
		return 1
	}
	serverName, err := enrollWithCode(cfg, raw, strings.TrimSpace(*agentHost))
	if err != nil {
		if *jsonOutput {
			data, _ := json.Marshal(map[string]interface{}{"ok": false, "error": err.Error()})
			fmt.Println(string(data))
		} else {
			fmt.Fprintf(os.Stderr, "配对失败：%v\n", err)
		}
		if !*noDialog {
			showErrorDialog("Mode Go 配对失败", err.Error())
		}
		return 1
	}
	if *jsonOutput {
		data, _ := json.Marshal(map[string]interface{}{"ok": true, "serverName": serverName})
		fmt.Println(string(data))
	} else {
		fmt.Printf("enrolled=true\nmcpServer=%s\n", serverName)
	}
	if !*noDialog {
		showInfoDialog("Mode Go 配对成功", "已登记到 Mode 桌面端。\n请回到远程控制页点击「刷新列表」。")
	}
	return 0
}

func parseEnrollTarget(raw string) (enrollTarget, error) {
	raw = strings.TrimSpace(raw)
	if strings.HasPrefix(strings.ToLower(raw), "mode-agent://") {
		parsed, err := url.Parse(raw)
		if err != nil {
			return enrollTarget{}, fmt.Errorf("配对串格式无效")
		}
		host := strings.TrimSpace(parsed.Query().Get("host"))
		port, _ := strconv.Atoi(parsed.Query().Get("port"))
		code := normalizeEnrollCode(parsed.Query().Get("code"))
		if host == "" || port <= 0 || port > 65535 || code == "" {
			return enrollTarget{}, fmt.Errorf("配对串缺少地址、端口或配对码")
		}
		return enrollTarget{Host: host, Port: port, Code: code}, nil
	}
	fields := strings.Fields(raw)
	if len(fields) < 2 {
		return enrollTarget{}, fmt.Errorf("需要 桌面地址:端口 配对码")
	}
	code := normalizeEnrollCode(fields[len(fields)-1])
	address := fields[0]
	host, portText, err := net.SplitHostPort(address)
	if err != nil {
		index := strings.LastIndex(address, ":")
		if index <= 0 {
			return enrollTarget{}, fmt.Errorf("桌面地址需要包含端口")
		}
		host = address[:index]
		portText = address[index+1:]
	}
	port, err := strconv.Atoi(portText)
	if err != nil || port <= 0 || port > 65535 || host == "" || code == "" {
		return enrollTarget{}, fmt.Errorf("配对信息格式无效")
	}
	return enrollTarget{Host: host, Port: port, Code: code}, nil
}

func normalizeEnrollCode(value string) string {
	var builder strings.Builder
	for _, char := range strings.ToUpper(value) {
		if (char >= '0' && char <= '9') || (char >= 'A' && char <= 'Z') {
			builder.WriteRune(char)
		}
	}
	code := builder.String()
	if len(code) < 4 || len(code) > 32 {
		return ""
	}
	return code
}

// enrollWithCode 支持三种输入：
//   - 6 位配对码：自动发现同一 LAN/Tailscale 内的桌面端并逐个尝试；
//   - mode-agent://enroll?... 完整配对串；
//   - 桌面地址:端口 配对码（CLI 兼容）。
func enrollWithCode(cfg *Config, raw, agentHost string) (string, error) {
	trimmed := strings.TrimSpace(raw)
	if trimmed == "" {
		return "", fmt.Errorf("请输入桌面端生成的 6 位配对码")
	}
	if strings.HasPrefix(strings.ToLower(trimmed), "mode-agent://") || len(strings.Fields(trimmed)) >= 2 {
		return enrollWithDesktop(cfg, trimmed, agentHost)
	}
	code := normalizeEnrollCode(trimmed)
	if len(code) != 6 {
		return "", fmt.Errorf("配对码应为 6 位")
	}
	peers := discoverDesktops(1800 * time.Millisecond)
	if len(peers) == 0 {
		return "", fmt.Errorf("未发现 Mode 桌面端；请确认与桌面在同一局域网或 Tailscale，并已开启局域网访问")
	}
	var lastErr error
	for _, peer := range peers {
		serverName, err := enrollWithTarget(cfg, enrollTarget{Host: peer.Host, Port: peer.Port, Code: code}, agentHost)
		if err == nil {
			return serverName, nil
		}
		lastErr = err
	}
	if lastErr == nil {
		lastErr = fmt.Errorf("没有可用的桌面端")
	}
	return "", fmt.Errorf("配对失败：%v", lastErr)
}

func enrollWithDesktop(cfg *Config, raw, agentHost string) (string, error) {
	target, err := parseEnrollTarget(raw)
	if err != nil {
		return "", err
	}
	return enrollWithTarget(cfg, target, agentHost)
}

func enrollWithTarget(cfg *Config, target enrollTarget, agentHost string) (string, error) {
	if strings.TrimSpace(agentHost) == "" {
		agentHost = primaryPairAddress()
	}
	name := hostnameOr("Mode Go")
	payload := enrollmentPayload{
		Code:      target.Code,
		AgentID:   cfg.AgentID,
		Name:      name,
		Host:      agentHost,
		Port:      cfg.Port,
		Token:     cfg.Token,
		Version:   version,
		Platform:  "windows",
		Addresses: localIPv4Addresses(),
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return "", err
	}
	client := &http.Client{Timeout: 15 * time.Second}
	request, err := http.NewRequest(
		"POST",
		fmt.Sprintf("http://%s:%d%s", target.Host, target.Port, remoteAgentEnrollPath),
		bytes.NewReader(body),
	)
	if err != nil {
		return "", err
	}
	request.Header.Set("Content-Type", "application/json")
	response, err := client.Do(request)
	if err != nil {
		return "", err
	}
	defer response.Body.Close()
	data, err := ioutil.ReadAll(response.Body)
	if err != nil {
		return "", err
	}
	var reply enrollmentReply
	if err := json.Unmarshal(data, &reply); err != nil {
		return "", fmt.Errorf("桌面端响应无效：HTTP %d", response.StatusCode)
	}
	if !reply.OK {
		if reply.Error == "" {
			reply.Error = fmt.Sprintf("HTTP %d", response.StatusCode)
		}
		return "", fmt.Errorf("%s", reply.Error)
	}
	return reply.ServerName, nil
}
