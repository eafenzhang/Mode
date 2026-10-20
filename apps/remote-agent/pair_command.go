package main

import (
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

func cmdPair(args []string) int {
	fs := flag.NewFlagSet("pair", flag.ContinueOnError)
	fs.SetOutput(os.Stderr)
	configFlag := fs.String("config", "", "config file path")
	noDialog := fs.Bool("no-dialog", false, "print the pairing code without a dialog")
	jsonOutput := fs.Bool("json", false, "print the raw pairing result")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	path := resolveConfigPath(*configFlag)
	cfg, err := loadConfig(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "%v\n", err)
		return 1
	}
	code, err := requestPairCode(cfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "无法生成配对码：%v\n请先运行 `mode-agent serve --config %s`\n", err, path)
		return 1
	}
	if *jsonOutput {
		data, _ := json.MarshalIndent(code, "", "  ")
		fmt.Println(string(data))
	} else {
		printPairInfo(cfg, code)
	}
	if !*noDialog {
		showPairCodeDialog(cfg, code)
	}
	return 0
}

func localAgentBaseURL(cfg *Config) string {
	host := strings.TrimSpace(cfg.Bind)
	if host == "" || host == "0.0.0.0" || host == "::" || host == "[::]" {
		host = "127.0.0.1"
	}
	return "http://" + net.JoinHostPort(host, strconv.Itoa(cfg.Port))
}

func requestPairCode(cfg *Config) (*PairCodeResult, error) {
	client := &http.Client{Timeout: 5 * time.Second}
	request, err := http.NewRequest("POST", localAgentBaseURL(cfg)+"/v1/pair-code", nil)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Authorization", "Bearer "+cfg.Token)
	response, err := client.Do(request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	data, err := ioutil.ReadAll(response.Body)
	if err != nil {
		return nil, err
	}
	var envelope struct {
		OK     bool           `json:"ok"`
		Result PairCodeResult `json:"result"`
		Error  *APIError      `json:"error"`
	}
	if err := json.Unmarshal(data, &envelope); err != nil {
		return nil, fmt.Errorf("invalid response: %v", err)
	}
	if !envelope.OK {
		if envelope.Error != nil {
			return nil, fmt.Errorf("%s: %s", envelope.Error.Code, envelope.Error.Message)
		}
		return nil, fmt.Errorf("agent returned an error")
	}
	return &envelope.Result, nil
}

func printPairInfo(cfg *Config, code *PairCodeResult) {
	host := primaryPairAddress()
	fmt.Printf("pairCode=%s\n", code.Code)
	fmt.Printf("expiresAt=%s\n", time.Unix(0, code.ExpiresAt*int64(time.Millisecond)).Format(time.RFC3339))
	fmt.Printf("name=%s\n", code.Name)
	fmt.Printf("port=%d\n", code.Port)
	fmt.Printf("addresses=%s\n", strings.Join(code.Addresses, ","))
	fmt.Printf("pairUri=%s\n", buildPairURI(host, code.Port, code.Code))
}

func buildPairURI(host string, port int, code string) string {
	query := url.Values{}
	query.Set("host", host)
	query.Set("port", strconv.Itoa(port))
	query.Set("code", code)
	return "mode-agent://pair?" + query.Encode()
}

func formatPairMessage(cfg *Config, code *PairCodeResult) string {
	host := primaryPairAddress()
	return fmt.Sprintf(
		"Mode Agent 已安装并启动。\n\n"+
			"在 Mode 桌面端打开「远程控制」→「添加被控端」：\n"+
			"地址：%s\n端口：%d\n配对码：%s\n\n"+
			"也可以直接粘贴：\n%s\n\n"+
			"配对码 10 分钟内有效。",
		host, code.Port, code.Code, buildPairURI(host, code.Port, code.Code),
	)
}
