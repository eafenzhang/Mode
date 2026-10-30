//go:build go1.13
// +build go1.13

package main

import (
	"bytes"
	"crypto"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"flag"
	"fmt"
	"io/ioutil"
	"net/http"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"
)

// Mode Cloud 登录（docs/specs/cloud-personal-accounts.md，v2）：
// 仅 GitHub Device Flow——显示 user code 并自动打开授权页，授权后换取云会话，
// 再用本机 API 令牌 enroll 到个人账号目录。同账号主控经目录直连，无 6 位配对码。

const (
	cloudDefaultBaseURL = "https://relay.mymode.cloud"
	// GitHub OAuth App 的 public client_id（Device Flow 不需要 secret）；
	// 可用 MODE_CLOUD_GITHUB_CLIENT_ID 覆盖（私有化/代理部署）。
	githubDefaultClientID = "Ov23liB79UcD0jvvzKkl"
	githubDeviceCodeURL   = "https://github.com/login/device/code"
	githubTokenURL        = "https://github.com/login/oauth/access_token"
	githubScope           = "read:user"

	githubPendingIntervalMS = 5000
	githubSlowDownPenaltyMS = 5000
	githubLoginDeadline     = 15 * time.Minute
)

type githubDeviceCodeReply struct {
	DeviceCode       string `json:"device_code"`
	UserCode         string `json:"user_code"`
	VerificationURI  string `json:"verification_uri"`
	Interval         int    `json:"interval"`
	ExpiresIn        int    `json:"expires_in"`
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description"`
}

type githubTokenReply struct {
	AccessToken      string `json:"access_token"`
	Error            string `json:"error"`
	ErrorDescription string `json:"error_description"`
}

type cloudSessionReply struct {
	SessionToken string `json:"sessionToken"`
	Error        string `json:"error"`
}

type cloudEnrollDevice struct {
	ID            string   `json:"id"`
	Name          string   `json:"name"`
	Platform      string   `json:"platform"`
	Version       string   `json:"version,omitempty"`
	PublicKey     string   `json:"publicKey"`
	RelayTunnelID string   `json:"relayTunnelId"`
	AccessToken   string   `json:"accessToken"`
	Addresses     []string `json:"addresses,omitempty"`
}

type cloudAssertion struct {
	Payload   string `json:"payload"`
	Signature string `json:"signature"`
}

type cloudEnrollRequest struct {
	Device    cloudEnrollDevice `json:"device"`
	Assertion cloudAssertion    `json:"assertion"`
	Nonce     string            `json:"nonce"`
	IssuedAt  int64             `json:"issuedAt"`
}

type cloudEnrollReply struct {
	AccountID   string `json:"accountId"`
	DeviceID    string `json:"deviceId"`
	DeviceToken string `json:"deviceToken"`
	RelayToken  string `json:"relayToken"`
	Error       string `json:"error"`
}

/** 本机 HTTP API 基址（安装器/守护进程与健康检查、关闭指令使用）。 */

func cloudBaseURL(cfg *Config) string {
	if trimmed := strings.TrimSpace(cfg.CloudBaseURL); trimmed != "" {
		return strings.TrimRight(trimmed, "/")
	}
	if overridden := strings.TrimSpace(os.Getenv("MODE_CLOUD_BASE_URL")); overridden != "" {
		return strings.TrimRight(overridden, "/")
	}
	return cloudDefaultBaseURL
}

func githubClientID() string {
	if overridden := strings.TrimSpace(os.Getenv("MODE_CLOUD_GITHUB_CLIENT_ID")); overridden != "" {
		return overridden
	}
	return githubDefaultClientID
}

func postJSON(url string, payload interface{}, token string, timeout time.Duration) ([]byte, int, error) {
	body, err := json.Marshal(payload)
	if err != nil {
		return nil, 0, err
	}
	request, err := http.NewRequest("POST", url, bytes.NewReader(body))
	if err != nil {
		return nil, 0, err
	}
	request.Header.Set("Content-Type", "application/json")
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	client := &http.Client{Timeout: timeout}
	response, err := client.Do(request)
	if err != nil {
		return nil, 0, err
	}
	defer response.Body.Close()
	data, err := ioutil.ReadAll(response.Body)
	if err != nil {
		return nil, response.StatusCode, err
	}
	return data, response.StatusCode, nil
}

// loadOrCreateCloudDeviceKey 返回 Ed25519 设备身份（私钥只落本机配置文件）。
func loadOrCreateCloudDeviceKey(cfg *Config) (ed25519.PublicKey, ed25519.PrivateKey, string, error) {
	if strings.TrimSpace(cfg.CloudDeviceKey) != "" {
		block, _ := pem.Decode([]byte(cfg.CloudDeviceKey))
		if block != nil {
			parsed, err := x509.ParsePKCS8PrivateKey(block.Bytes)
			if err == nil {
				if key, ok := parsed.(ed25519.PrivateKey); ok {
					publicKeyPEM, err := marshalCloudPublicKey(key.Public())
					if err != nil {
						return nil, nil, "", err
					}
					return key.Public().(ed25519.PublicKey), key, publicKeyPEM, nil
				}
			}
		}
		// 私钥损坏：重新生成（enroll 会覆盖云端设备记录）。
	}
	publicKey, privateKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, nil, "", err
	}
	privateDER, err := x509.MarshalPKCS8PrivateKey(privateKey)
	if err != nil {
		return nil, nil, "", err
	}
	privatePEM := string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: privateDER}))
	publicKeyPEM, err := marshalCloudPublicKey(publicKey)
	if err != nil {
		return nil, nil, "", err
	}
	cfg.CloudDeviceKey = privatePEM
	return publicKey, privateKey, publicKeyPEM, nil
}

func marshalCloudPublicKey(key crypto.PublicKey) (string, error) {
	der, err := x509.MarshalPKIXPublicKey(key)
	if err != nil {
		return "", err
	}
	return string(pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: der})), nil
}

func cloudRelayTunnelID(cfg *Config) string {
	if trimmed := strings.TrimSpace(cfg.CloudRelayTunnelID); trimmed != "" {
		return trimmed
	}
	raw := make([]byte, 24)
	if _, err := rand.Read(raw); err != nil {
		return ""
	}
	cfg.CloudRelayTunnelID = base64.RawURLEncoding.EncodeToString(raw)
	return cfg.CloudRelayTunnelID
}

// canonicalCloudPayload 与云端 canonicalJson 对齐：按 key 排序的 JSON（map 序列化即有序）。
func canonicalCloudPayload(deviceID, relayTunnelID, nonce string, issuedAt int64) ([]byte, error) {
	return json.Marshal(map[string]interface{}{
		"device": map[string]interface{}{
			"id":            deviceID,
			"relayTunnelId": relayTunnelID,
		},
		"issuedAt": issuedAt,
		"nonce":    nonce,
	})
}

/** 打开系统浏览器（Windows 缺省 rundll32；失败时用户手动打开链接）。 */
func openInBrowser(uri string) {
	if strings.TrimSpace(uri) == "" {
		return
	}
	if runtime.GOOS == "windows" {
		exec.Command("rundll32", "url.dll,FileProtocolHandler", uri).Start()
		return
	}
	exec.Command("xdg-open", uri).Start()
}

/** 换取云会话（GitHub access token → sessionToken）。 */
func exchangeCloudSession(cfg *Config, githubToken string) (string, error) {
	data, status, err := postJSON(
		cloudBaseURL(cfg)+"/v1/auth/github",
		map[string]string{
			"accessToken": githubToken,
			"clientLabel": "mode-agent-" + runtime.GOOS,
		},
		"",
		15*time.Second,
	)
	if err != nil {
		return "", fmt.Errorf("连接 Mode Cloud 失败：%v", err)
	}
	var reply cloudSessionReply
	if err := json.Unmarshal(data, &reply); err != nil {
		return "", fmt.Errorf("Mode Cloud 响应无效：HTTP %d", status)
	}
	if status != 200 || reply.SessionToken == "" {
		if reply.Error == "invalid_github_token" {
			return "", fmt.Errorf("GitHub 授权无效，请重试")
		}
		return "", fmt.Errorf("Mode Cloud 登录失败（HTTP %d）", status)
	}
	return reply.SessionToken, nil
}

/** enroll 本机到个人账号目录：上报本机 API 令牌与地址，同账号主控即可直连。 */
func enrollCloudDevice(cfg *Config, sessionToken string) error {
	_, privateKey, publicKeyPEM, err := loadOrCreateCloudDeviceKey(cfg)
	if err != nil {
		return err
	}
	relayTunnelID := cloudRelayTunnelID(cfg)
	if relayTunnelID == "" {
		return fmt.Errorf("生成中继隧道 ID 失败")
	}
	deviceID := cfg.AgentID
	if strings.TrimSpace(deviceID) == "" {
		deviceID = cloudRelayTunnelID(cfg) + "-agent"
	}
	issuedAt := time.Now().UnixMilli()
	nonceRaw := make([]byte, 16)
	if _, err := rand.Read(nonceRaw); err != nil {
		return err
	}
	nonce := base64.RawURLEncoding.EncodeToString(nonceRaw)
	payload, err := canonicalCloudPayload(deviceID, relayTunnelID, nonce, issuedAt)
	if err != nil {
		return err
	}
	signature := ed25519.Sign(privateKey, payload)
	request := cloudEnrollRequest{
		Device: cloudEnrollDevice{
			ID:            deviceID,
			Name:          hostnameOr("Mode Go"),
			Platform:      runtime.GOOS,
			Version:       version,
			PublicKey:     publicKeyPEM,
			RelayTunnelID: relayTunnelID,
			AccessToken:   cfg.Token,
			Addresses:     localIPv4Addresses(),
		},
		Assertion: cloudAssertion{
			Payload:   string(payload),
			Signature: base64.StdEncoding.EncodeToString(signature),
		},
		Nonce:    nonce,
		IssuedAt: issuedAt,
	}
	data, status, err := postJSON(
		cloudBaseURL(cfg)+"/v1/devices/enroll",
		request,
		sessionToken,
		15*time.Second,
	)
	if err != nil {
		return fmt.Errorf("连接 Mode Cloud 失败：%v", err)
	}
	var reply cloudEnrollReply
	if err := json.Unmarshal(data, &reply); err != nil {
		return fmt.Errorf("Mode Cloud 响应无效：HTTP %d", status)
	}
	if status != 200 {
		switch reply.Error {
		case "device_conflict":
			return fmt.Errorf("该设备已属于其他账号；请先在旧账号撤销它")
		case "tunnel_conflict":
			return fmt.Errorf("中继隧道 ID 冲突，请重试")
		case "session_expired":
			return fmt.Errorf("登录会话已过期，请重新登录")
		default:
			return fmt.Errorf("设备注册失败（HTTP %d）", status)
		}
	}
	cfg.CloudDeviceToken = reply.DeviceToken
	return saveConfig(cfg, configPathOf(cfg))
}

/** 用设备令牌刷新会话并重新 enroll（serve 启动时后台维持在线状态与地址）。 */
func refreshCloudEnrollment(cfg *Config) error {
	sessionToken := strings.TrimSpace(cfg.CloudSessionToken)
	if sessionToken == "" {
		return nil
	}
	data, status, err := postJSON(
		cloudBaseURL(cfg)+"/v1/auth/refresh",
		map[string]string{},
		sessionToken,
		15*time.Second,
	)
	if err != nil || status != 200 {
		// 会话过期等失败：等用户重新 login；不清令牌（目录里设备还在）。
		return fmt.Errorf("刷新云会话失败（HTTP %d）", status)
	}
	var reply struct {
		SessionToken string `json:"sessionToken"`
	}
	if err := json.Unmarshal(data, &reply); err != nil || reply.SessionToken == "" {
		return fmt.Errorf("云会话刷新响应无效")
	}
	cfg.CloudSessionToken = reply.SessionToken
	if err := enrollCloudDevice(cfg, reply.SessionToken); err != nil {
		return err
	}
	return saveConfig(cfg, configPathOf(cfg))
}

func cmdCloudLogin(args []string) int {
	fs := flag.NewFlagSet("login", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
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
	result := runCloudLogin(cfg, path, *noDialog)
	if *jsonOutput {
		data, _ := json.Marshal(result)
		fmt.Println(string(data))
		return boolToInt(result.OK)
	}
	if !result.OK {
		fmt.Fprintf(os.Stderr, "%s\n", result.Error)
		if !*noDialog {
			showErrorDialog("Mode Go 登录失败", result.Error)
		}
		return 1
	}
	fmt.Printf("login=true\naccount=%s\n", result.Account)
	if !*noDialog {
		showInfoDialog("Mode Go 登录成功", "设备已加入你的 Mode Cloud 账号。\n在桌面端登录同一账号即可看到并连接这台设备。")
	}
	return 0
}

func boolToInt(value bool) int {
	if value {
		return 0
	}
	return 1
}

func runCloudLogin(cfg *Config, configPath string, noDialog bool) cloudLoginResult {
	// 1) 申请 device code。
	data, status, err := postJSON(
		githubDeviceCodeURL,
		map[string]string{"client_id": githubClientID(), "scope": githubScope},
		"",
		15*time.Second,
	)
	if err != nil {
		return cloudLoginResult{Error: fmt.Sprintf("无法连接 GitHub：%v", err)}
	}
	var deviceCode githubDeviceCodeReply
	if err := json.Unmarshal(data, &deviceCode); err != nil || status != 200 || deviceCode.DeviceCode == "" {
		detail := deviceCode.ErrorDescription
		if detail == "" {
			detail = fmt.Sprintf("HTTP %d", status)
		}
		return cloudLoginResult{Error: fmt.Sprintf("GitHub Device Flow 失败：%s", detail)}
	}

	// 2) 自动打开授权页；提示框只承载 user code（异步，不阻塞轮询）。
	openInBrowser(deviceCode.VerificationURI)
	if !noDialog {
		go showInfoDialog(
			"Mode Go 登录 GitHub",
			fmt.Sprintf("浏览器已打开 GitHub 授权页。\n\n如未打开，请访问：\n%s\n\n输入代码：%s",
				deviceCode.VerificationURI, deviceCode.UserCode),
		)
	}

	// 3) 轮询 token 端点（interval / slow_down / 过期）。
	interval := time.Duration(deviceCode.Interval) * time.Second
	if interval <= 0 {
		interval = githubPendingIntervalMS * time.Millisecond
	}
	deadline := time.Now().Add(githubLoginDeadline)
	if deviceCode.ExpiresIn > 0 &&
		time.Now().Add(time.Duration(deviceCode.ExpiresIn)*time.Second).Before(deadline) {
		deadline = time.Now().Add(time.Duration(deviceCode.ExpiresIn) * time.Second)
	}
	var githubToken string
	for {
		time.Sleep(interval)
		if time.Now().After(deadline) {
			return cloudLoginResult{Error: "GitHub 授权已过期，请重新运行 login"}
		}
		tokenData, _, tokenErr := postJSON(
			githubTokenURL,
			map[string]string{
				"client_id":   githubClientID(),
				"device_code": deviceCode.DeviceCode,
				"grant_type":  "urn:ietf:params:oauth:grant-type:device_code",
			},
			"",
			15*time.Second,
		)
		if tokenErr != nil {
			continue
		}
		var tokenReply githubTokenReply
		if err := json.Unmarshal(tokenData, &tokenReply); err != nil {
			continue
		}
		if tokenReply.AccessToken != "" {
			githubToken = tokenReply.AccessToken
			break
		}
		switch tokenReply.Error {
		case "authorization_pending":
			// 继续等待。
		case "slow_down":
			interval += githubSlowDownPenaltyMS * time.Millisecond
		case "expired_token":
			return cloudLoginResult{Error: "GitHub 授权已过期，请重新运行 login"}
		default:
			detail := tokenReply.ErrorDescription
			if detail == "" {
				detail = tokenReply.Error
			}
			return cloudLoginResult{Error: fmt.Sprintf("GitHub 登录失败：%s", detail)}
		}
	}

	// 4) 换云会话 → enroll 本机。
	sessionToken, err := exchangeCloudSession(cfg, githubToken)
	if err != nil {
		return cloudLoginResult{Error: err.Error()}
	}
	cfg.CloudSessionToken = sessionToken
	if err := enrollCloudDevice(cfg, sessionToken); err != nil {
		return cloudLoginResult{Error: err.Error()}
	}
	if err := saveConfig(cfg, configPath); err != nil {
		return cloudLoginResult{Error: fmt.Sprintf("保存配置失败：%v", err)}
	}
	return cloudLoginResult{OK: true, Account: hostnameOr("Mode Go")}
}

// maintainCloudEnrollment 在 serve 运行期间维持云目录状态：refresh 会话 +
// 重新 enroll（地址变化、重启后立即上线）。未登录时静默返回。
func maintainCloudEnrollment(cfg *Config) {
	if strings.TrimSpace(cfg.CloudSessionToken) == "" {
		return
	}
	if err := refreshCloudEnrollment(cfg); err != nil {
		fmt.Fprintf(os.Stderr, "cloud enrollment refresh: %v\n", err)
	}
	ticker := time.NewTicker(6 * time.Hour)
	defer ticker.Stop()
	for range ticker.C {
		if err := refreshCloudEnrollment(cfg); err != nil {
			fmt.Fprintf(os.Stderr, "cloud enrollment refresh: %v\n", err)
		}
	}
}
