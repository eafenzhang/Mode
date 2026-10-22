package main

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/draw"
	_ "image/jpeg"
	_ "image/png"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

const (
	panelWidthDIP      = 300
	panelHeightDIP     = 400
	panelMaxTitleRunes = 200
	panelMaxTextBytes  = 64 << 10
	panelMaxImageBytes = 4 << 20
	panelMaxBase64     = 8 << 20
)

// PanelContent 是桌面端写入面板的请求体。没有 HTML/URL 字段：被控端只渲染纯文本与图片，
// 避免把面板变成可执行内容宿主。
type PanelContent struct {
	Title         string `json:"title,omitempty"`
	Text          string `json:"text,omitempty"`
	ImageBase64   string `json:"imageBase64,omitempty"`
	ImageMimeType string `json:"imageMimeType,omitempty"`
}

type PanelSnapshot struct {
	Title     string
	Text      string
	Image     *image.RGBA
	HasImage  bool
	UpdatedAt time.Time
}

type PanelStatus struct {
	Visible    bool   `json:"visible"`
	Title      string `json:"title,omitempty"`
	TextLength int    `json:"textLength"`
	HasImage   bool   `json:"hasImage"`
	UpdatedAt  string `json:"updatedAt,omitempty"`
}

type panelController struct {
	mu        sync.Mutex
	title     string
	text      string
	image     *image.RGBA
	updatedAt time.Time
}

func newPanelController() *panelController {
	return &panelController{}
}

func (p *panelController) set(content PanelContent) (*PanelStatus, *APIError) {
	title := strings.TrimSpace(sanitizeUTF8([]byte(content.Title)))
	if utf8.RuneCountInString(title) > panelMaxTitleRunes {
		return nil, invalidArg("title is too long")
	}
	text := sanitizeUTF8([]byte(content.Text))
	if len(text) > panelMaxTextBytes {
		return nil, apiError("payload_too_large", "text is too large", 413)
	}
	decoded, apiErr := decodePanelImage(content.ImageBase64, content.ImageMimeType)
	if apiErr != nil {
		return nil, apiErr
	}
	if title == "" && text == "" && decoded == nil {
		return nil, invalidArg("title, text or imageBase64 is required")
	}
	p.mu.Lock()
	p.title = title
	p.text = text
	p.image = decoded
	p.updatedAt = time.Now()
	p.mu.Unlock()
	status := p.status()
	return &status, nil
}

func (p *panelController) clear() *PanelStatus {
	p.mu.Lock()
	p.title = ""
	p.text = ""
	p.image = nil
	p.updatedAt = time.Now()
	p.mu.Unlock()
	status := p.status()
	return &status
}

func (p *panelController) snapshot() PanelSnapshot {
	p.mu.Lock()
	defer p.mu.Unlock()
	return PanelSnapshot{
		Title:     p.title,
		Text:      p.text,
		Image:     p.image,
		HasImage:  p.image != nil,
		UpdatedAt: p.updatedAt,
	}
}

func (p *panelController) status() PanelStatus {
	p.mu.Lock()
	defer p.mu.Unlock()
	status := PanelStatus{
		Visible:    agentPanelVisible(),
		Title:      p.title,
		TextLength: len(p.text),
		HasImage:   p.image != nil,
	}
	if !p.updatedAt.IsZero() {
		status.UpdatedAt = nowRFC3339(p.updatedAt)
	}
	return status
}

func decodePanelImage(encoded, mimeType string) (*image.RGBA, *APIError) {
	encoded = stripDataURLPrefix(encoded)
	mimeType = strings.ToLower(strings.TrimSpace(mimeType))
	if mimeType != "" && mimeType != "image/png" && mimeType != "image/jpeg" {
		return nil, invalidArg("imageMimeType must be image/png or image/jpeg")
	}
	if encoded == "" {
		return nil, nil
	}
	if len(encoded) > panelMaxBase64 {
		return nil, apiError("payload_too_large", "imageBase64 is too large", 413)
	}
	raw, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		raw, err = base64.RawStdEncoding.DecodeString(encoded)
	}
	if err != nil {
		return nil, invalidArg("imageBase64 is not valid base64")
	}
	if len(raw) > panelMaxImageBytes {
		return nil, apiError("payload_too_large", "decoded image is too large", 413)
	}
	decoded, format, err := image.Decode(bytes.NewReader(raw))
	if err != nil {
		return nil, invalidArg("image data is not a supported PNG or JPEG")
	}
	if format != "png" && format != "jpeg" {
		return nil, invalidArg("image data is not a supported PNG or JPEG")
	}
	bounds := decoded.Bounds()
	if bounds.Dx() <= 0 || bounds.Dy() <= 0 {
		return nil, invalidArg("image has empty bounds")
	}
	rgba := image.NewRGBA(image.Rect(0, 0, bounds.Dx(), bounds.Dy()))
	draw.Draw(rgba, rgba.Bounds(), decoded, bounds.Min, draw.Src)
	return rgba, nil
}

func stripDataURLPrefix(value string) string {
	trimmed := strings.TrimSpace(value)
	lower := strings.ToLower(trimmed)
	index := strings.Index(lower, "base64,")
	if index < 0 || index > 128 || !strings.HasPrefix(lower, "data:image/") {
		return trimmed
	}
	return trimmed[index+len("base64,"):]
}
