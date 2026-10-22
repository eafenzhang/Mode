package main

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/png"
	"io/ioutil"
	"net/http/httptest"
	"os"
	"runtime"
	"strings"
	"testing"
)

func TestPanelContentLifecycle(t *testing.T) {
	panel := newPanelController()
	initial := panel.status()
	if initial.HasImage || initial.TextLength != 0 || initial.Title != "" || initial.Visible {
		t.Fatalf("expected blank panel, got %+v", initial)
	}
	status, apiErr := panel.set(PanelContent{Title: "标题", Text: "第一行\n第二行"})
	if apiErr != nil {
		t.Fatalf("set panel: %v", apiErr)
	}
	if status.Title != "标题" || status.TextLength == 0 || status.HasImage {
		t.Fatalf("unexpected status after set: %+v", status)
	}
	snapshot := panel.snapshot()
	if snapshot.Title != "标题" || !strings.Contains(snapshot.Text, "第二行") {
		t.Fatalf("unexpected snapshot: %+v", snapshot)
	}
	status = panel.clear()
	if status.Title != "" || status.TextLength != 0 || status.HasImage {
		t.Fatalf("expected blank panel after clear, got %+v", status)
	}
}

func TestPanelImageValidation(t *testing.T) {
	panel := newPanelController()
	pngData := encodeTestPNG(t)
	if _, apiErr := panel.set(PanelContent{ImageBase64: base64.StdEncoding.EncodeToString(pngData)}); apiErr != nil {
		t.Fatalf("valid png rejected: %v", apiErr)
	}
	if !panel.snapshot().HasImage {
		t.Fatalf("expected image to be stored")
	}
	if _, apiErr := panel.set(PanelContent{ImageBase64: "not-base64"}); apiErr == nil || apiErr.Code != "invalid_argument" {
		t.Fatalf("expected invalid base64, got %+v", apiErr)
	}
	if _, apiErr := panel.set(PanelContent{ImageBase64: base64.StdEncoding.EncodeToString([]byte("plain text"))}); apiErr == nil {
		t.Fatalf("expected non-image payload to be rejected")
	}
	if _, apiErr := panel.set(PanelContent{ImageBase64: "aaaa", ImageMimeType: "image/gif"}); apiErr == nil {
		t.Fatalf("expected unsupported mime type to be rejected")
	}
	if _, apiErr := panel.set(PanelContent{}); apiErr == nil {
		t.Fatalf("expected empty content to be rejected")
	}
	if _, apiErr := panel.set(PanelContent{Title: strings.Repeat("x", panelMaxTitleRunes+1)}); apiErr == nil {
		t.Fatalf("expected overlong title to be rejected")
	}
	if _, apiErr := panel.set(PanelContent{Text: strings.Repeat("x", panelMaxTextBytes+1)}); apiErr == nil {
		t.Fatalf("expected overlong text to be rejected")
	}
}

func TestPanelHTTPAndMCP(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("panel HTTP endpoints are Windows-only")
	}
	_, httpServer, token, cleanup := newPanelTestServer(t, true)
	defer cleanup()

	status, body := requestJSON(t, "GET", httpServer.URL+"/v1/panel", token, nil)
	if status != 200 || !strings.Contains(string(body), `"hasImage":false`) {
		t.Fatalf("panel status=%d body=%s", status, body)
	}
	status, body = requestJSON(t, "POST", httpServer.URL+"/v1/panel", token, map[string]string{
		"title": "远程标题",
		"text":  "桌面端写入的内容",
	})
	if status != 200 || !strings.Contains(string(body), `"title":"远程标题"`) {
		t.Fatalf("panel post status=%d body=%s", status, body)
	}
	status, body = requestJSON(t, "DELETE", httpServer.URL+"/v1/panel", token, nil)
	if status != 200 || !strings.Contains(string(body), `"textLength":0`) {
		t.Fatalf("panel delete status=%d body=%s", status, body)
	}

	mcpCall := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      1,
		"method":  "tools/call",
		"params": map[string]interface{}{
			"name": "remote_panel_show",
			"arguments": map[string]interface{}{
				"title": "MCP 标题",
				"text":  "通过 MCP 写入",
			},
		},
	}
	status, body = requestJSON(t, "POST", httpServer.URL+"/mcp", token, mcpCall)
	if status != 200 || !strings.Contains(string(body), "MCP 标题") {
		t.Fatalf("mcp panel call status=%d body=%s", status, body)
	}
}

func TestPanelHTTPRequiresDesktopControl(t *testing.T) {
	if runtime.GOOS != "windows" {
		t.Skip("panel HTTP endpoints are Windows-only")
	}
	_, httpServer, token, cleanup := newPanelTestServer(t, false)
	defer cleanup()
	status, body := requestJSON(t, "POST", httpServer.URL+"/v1/panel", token, map[string]string{"text": "x"})
	if status != 403 || !strings.Contains(string(body), "capability_disabled") {
		t.Fatalf("expected capability_disabled, status=%d body=%s", status, body)
	}
}

func newPanelTestServer(t *testing.T, allowDesktopControl bool) (*agentServer, *httptest.Server, string, func()) {
	t.Helper()
	dir, err := ioutil.TempDir("", "mode-agent-panel-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	cfg := &Config{
		Bind:                "127.0.0.1",
		Port:                47821,
		Token:               "test-token",
		AgentID:             "test-agent",
		DataDir:             dir,
		AllowShell:          true,
		AllowDesktopControl: allowDesktopControl,
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

func encodeTestPNG(t *testing.T) []byte {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, 2, 2))
	img.Set(0, 0, color.RGBA{R: 255, A: 255})
	img.Set(1, 1, color.RGBA{G: 255, A: 255})
	var buffer bytes.Buffer
	if err := png.Encode(&buffer, img); err != nil {
		t.Fatalf("encode png: %v", err)
	}
	return buffer.Bytes()
}
