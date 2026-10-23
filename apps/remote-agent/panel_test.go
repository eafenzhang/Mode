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

func TestPanelBlocksLifecycle(t *testing.T) {
	panel := newPanelController()
	content := PanelContent{
		Title: "GasHub v2.0.70",
		Blocks: []PanelBlock{
			{Kind: "stat", Label: "本日上传订单", Value: "0"},
			{Kind: "card", Title: "hub-monitor", Badge: &PanelBadge{Text: "已匹配", Tone: "success"}, Lines: []string{"SQLite", "10172026091809K3"}},
			{Kind: "card", Title: "oil_system1", Badge: &PanelBadge{Text: "未登录", Tone: "danger"}, Lines: []string{"Access", "1017121212"}},
		},
		Footer: &PanelFooter{Label: "一键同步", Action: "sync-all"},
	}
	status, apiErr := panel.set(content)
	if apiErr != nil {
		t.Fatalf("set blocks: %v", apiErr)
	}
	if !status.HasBlocks || status.FooterAction != "sync-all" {
		t.Fatalf("unexpected status: %+v", status)
	}
	snapshot := panel.snapshot()
	if len(snapshot.Blocks) != 3 || snapshot.Footer == nil || snapshot.Footer.Action != "sync-all" {
		t.Fatalf("unexpected snapshot blocks: %+v", snapshot)
	}
	if snapshot.Blocks[1].Badge == nil || snapshot.Blocks[1].Badge.Tone != "success" {
		t.Fatalf("badge not preserved: %+v", snapshot.Blocks[1])
	}
	status = panel.clear()
	if status.HasBlocks || status.FooterAction != "" {
		t.Fatalf("expected blocks cleared, got %+v", status)
	}
}

func TestPanelBlocksValidation(t *testing.T) {
	panel := newPanelController()
	valid := PanelContent{Blocks: []PanelBlock{{Kind: "card", Title: "a"}}}

	if _, apiErr := panel.set(PanelContent{Text: "x", Blocks: []PanelBlock{{Kind: "card", Title: "a"}}}); apiErr == nil {
		t.Fatalf("expected blocks+text rejection")
	}
	if _, apiErr := panel.set(PanelContent{ImageBase64: "aaaa", Blocks: []PanelBlock{{Kind: "card", Title: "a"}}}); apiErr == nil || apiErr.Code != "invalid_argument" {
		t.Fatalf("expected blocks+image rejection, got %+v", apiErr)
	}
	if _, apiErr := panel.set(PanelContent{Footer: &PanelFooter{Label: "go"}}); apiErr == nil {
		t.Fatalf("expected footer-without-blocks rejection")
	}
	if _, apiErr := panel.set(valid); apiErr != nil {
		t.Fatalf("valid blocks rejected: %v", apiErr)
	}
	if _, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "webview", Title: "x"}}}); apiErr == nil || !strings.Contains(apiErr.Message, "unknown block kind") {
		t.Fatalf("expected unknown kind rejection, got %+v", apiErr)
	}
	if _, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "card", Title: "x", Badge: &PanelBadge{Text: "b", Tone: "blue"}}}}); apiErr == nil || !strings.Contains(apiErr.Message, "unknown badge tone") {
		t.Fatalf("expected unknown tone rejection, got %+v", apiErr)
	}
	if _, apiErr := panel.set(PanelContent{
		Blocks: []PanelBlock{{Kind: "card", Title: "x"}},
		Footer: &PanelFooter{Label: "go", Action: "has space"},
	}); apiErr == nil {
		t.Fatalf("expected invalid action rejection")
	}
	if _, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "stat", Value: ""}}}); apiErr == nil {
		t.Fatalf("expected empty stat value rejection")
	}
	if _, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "card", Title: "x", Lines: []string{"1", "2", "3", "4", "5"}}}}); apiErr == nil {
		t.Fatalf("expected >4 lines rejection")
	}
	tooMany := make([]PanelBlock, panelMaxBlocks+1)
	for i := range tooMany {
		tooMany[i] = PanelBlock{Kind: "card", Title: "x"}
	}
	if _, apiErr := panel.set(PanelContent{Blocks: tooMany}); apiErr == nil {
		t.Fatalf("expected >24 blocks rejection")
	}
	if _, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "card", Title: "x\x00y"}}}); apiErr == nil {
		t.Fatalf("expected NUL rejection")
	}
	// tone 省略默认 muted
	status, apiErr := panel.set(PanelContent{Blocks: []PanelBlock{{Kind: "card", Title: "x", Badge: &PanelBadge{Text: "b"}}}})
	if apiErr != nil {
		t.Fatalf("default tone rejected: %v", apiErr)
	}
	if !status.HasBlocks {
		t.Fatalf("expected default tone accepted")
	}
	if got := panel.snapshot().Blocks[0].Badge.Tone; got != "muted" {
		t.Fatalf("expected muted default, got %q", got)
	}
}

func TestPanelActionQueue(t *testing.T) {
	panel := newPanelController()
	// 非法 action 直接丢弃，不入队
	panel.pushAction("has space")
	panel.pushAction("")
	if actions := panel.takeActions(true); len(actions) != 0 {
		t.Fatalf("expected no actions, got %+v", actions)
	}
	for i := 0; i < panelMaxActions+4; i++ {
		panel.pushAction("act-" + string(rune('a'+i)))
	}
	// peek 不消费
	peeked := panel.takeActions(false)
	if len(peeked) != panelMaxActions {
		t.Fatalf("expected %d actions on peek, got %d", panelMaxActions, len(peeked))
	}
	if again := panel.takeActions(false); len(again) != panelMaxActions {
		t.Fatalf("peek should not consume, got %d", len(again))
	}
	// 满 16 条，最旧的被丢弃
	if peeked[0].Action != "act-e" {
		t.Fatalf("expected oldest dropped, first=%q", peeked[0].Action)
	}
	consumed := panel.takeActions(true)
	if len(consumed) != panelMaxActions {
		t.Fatalf("expected %d consumed, got %d", panelMaxActions, len(consumed))
	}
	if after := panel.takeActions(true); len(after) != 0 {
		t.Fatalf("expected empty after consume, got %d", len(after))
	}
	// clear 清空队列
	panel.pushAction("keep-me")
	panel.clear()
	if leftovers := panel.takeActions(true); len(leftovers) != 0 {
		t.Fatalf("expected clear to empty queue, got %+v", leftovers)
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
