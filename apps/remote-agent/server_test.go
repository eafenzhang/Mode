package main

import (
	"bytes"
	"encoding/json"
	"io/ioutil"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime"
	"strings"
	"testing"
)

func newTestServer(t *testing.T) (*agentServer, *httptest.Server, string, func()) {
	t.Helper()
	dir, err := ioutil.TempDir("", "mode-agent-server-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	cfg := &Config{
		Bind:                "127.0.0.1",
		Port:                47821,
		Token:               "test-token",
		AgentID:             "test-agent",
		DataDir:             dir,
		AllowedRoots:        []string{dir},
		AllowShell:          true,
		AllowWrite:          true,
		AllowDelete:         true,
		AllowDesktopControl: false,
		AllowDB:             false,
		DSNs:                map[string]string{},
	}
	server, err := newAgentServer(cfg)
	if err != nil {
		t.Fatalf("new server: %v", err)
	}
	httpServer := httptest.NewServer(server)
	cleanup := func() {
		httpServer.Close()
		server.close()
		os.RemoveAll(dir)
	}
	return server, httpServer, cfg.Token, cleanup
}

func requestJSON(t *testing.T, method, url, token string, body interface{}) (int, []byte) {
	t.Helper()
	var reader *bytes.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			t.Fatalf("marshal body: %v", err)
		}
		reader = bytes.NewReader(data)
	} else {
		reader = bytes.NewReader(nil)
	}
	request, err := http.NewRequest(method, url, reader)
	if err != nil {
		t.Fatalf("new request: %v", err)
	}
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		t.Fatalf("request: %v", err)
	}
	defer response.Body.Close()
	data, err := ioutil.ReadAll(response.Body)
	if err != nil {
		t.Fatalf("read body: %v", err)
	}
	return response.StatusCode, data
}

func TestHealthAndAuth(t *testing.T) {
	_, httpServer, token, cleanup := newTestServer(t)
	defer cleanup()
	status, body := requestJSON(t, "GET", httpServer.URL+"/health", "", nil)
	if status != 200 || !strings.Contains(string(body), `"ok":true`) {
		t.Fatalf("health status=%d body=%s", status, body)
	}
	status, _ = requestJSON(t, "GET", httpServer.URL+"/v1/info", "", nil)
	if status != 401 {
		t.Fatalf("expected 401 without token, got %d", status)
	}
	status, body = requestJSON(t, "GET", httpServer.URL+"/v1/info", token, nil)
	if status != 200 || !strings.Contains(string(body), `"agent":"mode-agent"`) {
		t.Fatalf("info status=%d body=%s", status, body)
	}
}

func TestExecAndFSOverHTTP(t *testing.T) {
	_, httpServer, token, cleanup := newTestServer(t)
	defer cleanup()
	command := map[string]interface{}{"command": "echo agent-http-test", "shell": "cmd", "timeoutMs": 10000}
	if runtime.GOOS != "windows" {
		command = map[string]interface{}{"command": "echo", "args": []string{"agent-http-test"}, "shell": "none", "timeoutMs": 10000}
	}
	status, body := requestJSON(t, "POST", httpServer.URL+"/v1/exec", token, command)
	if status != 200 || !strings.Contains(string(body), "agent-http-test") {
		t.Fatalf("exec status=%d body=%s", status, body)
	}
}

func TestPairingFlow(t *testing.T) {
	_, httpServer, token, cleanup := newTestServer(t)
	defer cleanup()
	status, body := requestJSON(t, "POST", httpServer.URL+"/v1/pair-code", token, map[string]interface{}{})
	if status != 200 || !strings.Contains(string(body), `"code"`) {
		t.Fatalf("pair-code status=%d body=%s", status, body)
	}
	var envelope struct {
		OK     bool           `json:"ok"`
		Result PairCodeResult `json:"result"`
	}
	if err := json.Unmarshal(body, &envelope); err != nil || !envelope.OK {
		t.Fatalf("decode pair-code: %v body=%s", err, body)
	}
	pairBody := map[string]interface{}{"code": envelope.Result.Code, "label": "test-desktop"}
	status, body = requestJSON(t, "POST", httpServer.URL+"/v1/pair", "", pairBody)
	if status != 200 || !strings.Contains(string(body), `"agentId":"test-agent"`) || !strings.Contains(string(body), `"token":"`+token+`"`) {
		t.Fatalf("pair status=%d body=%s", status, body)
	}
	status, _ = requestJSON(t, "POST", httpServer.URL+"/v1/pair", "", pairBody)
	if status != 401 {
		t.Fatalf("expected one-time code to be consumed, got %d", status)
	}
	preflight, err := http.NewRequest("OPTIONS", httpServer.URL+"/v1/pair", nil)
	if err != nil {
		t.Fatalf("preflight request: %v", err)
	}
	preflight.Header.Set("Origin", "http://localhost:3000")
	response, err := http.DefaultClient.Do(preflight)
	if err != nil {
		t.Fatalf("preflight: %v", err)
	}
	response.Body.Close()
	if response.StatusCode != 204 || response.Header.Get("Access-Control-Allow-Origin") != "*" {
		t.Fatalf("preflight status=%d cors=%q", response.StatusCode, response.Header.Get("Access-Control-Allow-Origin"))
	}
}

func TestMCPInitializeAndToolList(t *testing.T) {
	_, httpServer, token, cleanup := newTestServer(t)
	defer cleanup()
	initialize := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      1,
		"method":  "initialize",
		"params": map[string]interface{}{
			"protocolVersion": "2025-11-25",
			"capabilities":    map[string]interface{}{},
			"clientInfo":      map[string]string{"name": "test", "version": "1"},
		},
	}
	status, body := requestJSON(t, "POST", httpServer.URL+"/mcp", token, initialize)
	if status != 200 || !strings.Contains(string(body), `"protocolVersion":"2025-11-25"`) {
		t.Fatalf("initialize status=%d body=%s", status, body)
	}
	listTools := map[string]interface{}{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": map[string]interface{}{}}
	status, body = requestJSON(t, "POST", httpServer.URL+"/mcp", token, listTools)
	if status != 200 || !strings.Contains(string(body), `"remote_info"`) || !strings.Contains(string(body), `"remote_exec"`) {
		t.Fatalf("tools/list status=%d body=%s", status, body)
	}
}
