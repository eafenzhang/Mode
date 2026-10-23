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

	// 声明式 UI 块（docs/specs/mode-agent-tray-panel.md「内容模型」）：
	// 块类型与 tone 都是闭集，只做确定性校验，不降级猜测。
	panelMaxBlocks           = 24
	panelMaxBlockLines       = 4
	panelMaxActions          = 16
	panelMaxStatLabelRunes   = 100
	panelMaxStatValueRunes   = 32
	panelMaxCardTitleRunes   = 200
	panelMaxLineRunes        = 200
	panelMaxBadgeTextRunes   = 16
	panelMaxFooterLabelRunes = 24
	panelMaxActionRunes      = 64
)

// PanelContent 是桌面端写入面板的请求体。没有 HTML/URL 字段：被控端只渲染纯文本、
// 图片与本仓库定义的受控声明式块，避免把面板变成可执行内容宿主。
type PanelContent struct {
	Title         string       `json:"title,omitempty"`
	Text          string       `json:"text,omitempty"`
	ImageBase64   string       `json:"imageBase64,omitempty"`
	ImageMimeType string       `json:"imageMimeType,omitempty"`
	Blocks        []PanelBlock `json:"blocks,omitempty"`
	Footer        *PanelFooter `json:"footer,omitempty"`
}

// PanelBlock 是结构化视图里的一个内容块；字段按 Kind 二选一使用。
type PanelBlock struct {
	Kind  string      `json:"kind"`
	Label string      `json:"label,omitempty"`
	Value string      `json:"value,omitempty"`
	Title string      `json:"title,omitempty"`
	Badge *PanelBadge `json:"badge,omitempty"`
	Lines []string    `json:"lines,omitempty"`
}

type PanelBadge struct {
	Text string `json:"text"`
	Tone string `json:"tone,omitempty"`
}

type PanelFooter struct {
	Label  string `json:"label"`
	Action string `json:"action,omitempty"`
}

// PanelAction 是 footer 按钮点击产生的事件；agent 只入队，不执行任何业务语义。
type PanelAction struct {
	Action string `json:"action"`
	At     string `json:"at"`
}

type queuedPanelAction struct {
	action string
	at     time.Time
}

type PanelSnapshot struct {
	Title     string
	Text      string
	Image     *image.RGBA
	HasImage  bool
	Blocks    []PanelBlock
	Footer    *PanelFooter
	UpdatedAt time.Time
}

type PanelStatus struct {
	Visible      bool   `json:"visible"`
	Title        string `json:"title,omitempty"`
	TextLength   int    `json:"textLength"`
	HasImage     bool   `json:"hasImage"`
	HasBlocks    bool   `json:"hasBlocks"`
	FooterAction string `json:"footerAction,omitempty"`
	UpdatedAt    string `json:"updatedAt,omitempty"`
}

type panelController struct {
	mu        sync.Mutex
	title     string
	text      string
	image     *image.RGBA
	blocks    []PanelBlock
	footer    *PanelFooter
	actions   []queuedPanelAction
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
	blocks, footer, apiErr := validatePanelBlocks(content)
	if apiErr != nil {
		return nil, apiErr
	}
	if title == "" && text == "" && decoded == nil && len(blocks) == 0 {
		return nil, invalidArg("title, text, imageBase64 or blocks is required")
	}
	p.mu.Lock()
	p.title = title
	p.text = text
	p.image = decoded
	p.blocks = blocks
	p.footer = footer
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
	p.blocks = nil
	p.footer = nil
	p.actions = nil
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
		Blocks:    p.blocks,
		Footer:    p.footer,
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
		HasBlocks:  len(p.blocks) > 0,
	}
	if p.footer != nil {
		status.FooterAction = p.footer.Action
	}
	if !p.updatedAt.IsZero() {
		status.UpdatedAt = nowRFC3339(p.updatedAt)
	}
	return status
}

// pushAction 由面板 UI 线程在 footer 点击时调用。队列仅内存、上限 16、满则丢最旧。
func (p *panelController) pushAction(action string) {
	action = strings.TrimSpace(action)
	if action == "" || !validPanelAction(action) {
		return
	}
	now := time.Now()
	p.mu.Lock()
	p.actions = append(p.actions, queuedPanelAction{action: action, at: now})
	if len(p.actions) > panelMaxActions {
		p.actions = p.actions[len(p.actions)-panelMaxActions:]
	}
	p.mu.Unlock()
}

// takeActions 拉取动作队列；consume=false 时只读不消费（peek）。
func (p *panelController) takeActions(consume bool) []PanelAction {
	p.mu.Lock()
	defer p.mu.Unlock()
	if len(p.actions) == 0 {
		return []PanelAction{}
	}
	result := make([]PanelAction, 0, len(p.actions))
	for _, queued := range p.actions {
		result = append(result, PanelAction{
			Action: queued.action,
			At:     nowRFC3339(queued.at),
		})
	}
	if consume {
		p.actions = nil
	}
	return result
}

// validatePanelBlocks 校验结构化视图载荷并返回深拷贝后的 blocks/footer。
// blocks 与 text/imageBase64 互斥；未知 kind/tone、非法 action 一律拒绝。
func validatePanelBlocks(content PanelContent) ([]PanelBlock, *PanelFooter, *APIError) {
	hasFooter := content.Footer != nil
	if len(content.Blocks) == 0 {
		if hasFooter {
			return nil, nil, invalidArg("footer requires at least one block")
		}
		return nil, nil, nil
	}
	if content.Text != "" || content.ImageBase64 != "" {
		return nil, nil, invalidArg("blocks cannot be combined with text or imageBase64")
	}
	if len(content.Blocks) > panelMaxBlocks {
		return nil, nil, invalidArg("blocks exceeds the limit of 24")
	}
	blocks := make([]PanelBlock, 0, len(content.Blocks))
	for index, block := range content.Blocks {
		validated, apiErr := validatePanelBlock(block, index)
		if apiErr != nil {
			return nil, nil, apiErr
		}
		blocks = append(blocks, validated)
	}
	var footer *PanelFooter
	if hasFooter {
		validated, apiErr := validatePanelFooter(*content.Footer)
		if apiErr != nil {
			return nil, nil, apiErr
		}
		footer = &validated
	}
	return blocks, footer, nil
}

func validatePanelBlock(block PanelBlock, index int) (PanelBlock, *APIError) {
	switch block.Kind {
	case "stat":
		if strings.TrimSpace(block.Value) == "" {
			return PanelBlock{}, invalidArg("stat block requires a non-empty value")
		}
		if err := checkPanelText(block.Label, panelMaxStatLabelRunes); err != nil {
			return PanelBlock{}, err
		}
		if err := checkPanelText(block.Value, panelMaxStatValueRunes); err != nil {
			return PanelBlock{}, err
		}
		if block.Title != "" || block.Badge != nil || len(block.Lines) > 0 {
			return PanelBlock{}, invalidArg("stat block does not accept title, badge or lines")
		}
		return PanelBlock{
			Kind:  "stat",
			Label: strings.TrimSpace(block.Label),
			Value: strings.TrimSpace(block.Value),
		}, nil
	case "card":
		title := strings.TrimSpace(block.Title)
		if title == "" {
			return PanelBlock{}, invalidArg("card block requires a non-empty title")
		}
		if err := checkPanelText(title, panelMaxCardTitleRunes); err != nil {
			return PanelBlock{}, err
		}
		if len(block.Lines) > panelMaxBlockLines {
			return PanelBlock{}, invalidArg("card block accepts at most 4 lines")
		}
		lines := make([]string, 0, len(block.Lines))
		for _, line := range block.Lines {
			line = strings.TrimSpace(sanitizeUTF8([]byte(line)))
			if err := checkPanelText(line, panelMaxLineRunes); err != nil {
				return PanelBlock{}, err
			}
			lines = append(lines, line)
		}
		var badge *PanelBadge
		if block.Badge != nil {
			text := strings.TrimSpace(sanitizeUTF8([]byte(block.Badge.Text)))
			if text == "" {
				return PanelBlock{}, invalidArg("badge requires non-empty text")
			}
			if err := checkPanelText(text, panelMaxBadgeTextRunes); err != nil {
				return PanelBlock{}, err
			}
			tone := strings.TrimSpace(block.Badge.Tone)
			if tone == "" {
				tone = "muted"
			}
			switch tone {
			case "success", "danger", "warning", "muted":
			default:
				return PanelBlock{}, invalidArg("unknown badge tone: " + tone)
			}
			badge = &PanelBadge{Text: text, Tone: tone}
		}
		if block.Label != "" || block.Value != "" {
			return PanelBlock{}, invalidArg("card block does not accept label or value")
		}
		return PanelBlock{
			Kind:  "card",
			Title: title,
			Badge: badge,
			Lines: lines,
		}, nil
	default:
		kind := strings.TrimSpace(block.Kind)
		if kind == "" {
			return PanelBlock{}, invalidArg("block kind is required")
		}
		return PanelBlock{}, invalidArg("unknown block kind: " + kind)
	}
}

func validatePanelFooter(footer PanelFooter) (PanelFooter, *APIError) {
	label := strings.TrimSpace(sanitizeUTF8([]byte(footer.Label)))
	if label == "" {
		return PanelFooter{}, invalidArg("footer requires a non-empty label")
	}
	if err := checkPanelText(label, panelMaxFooterLabelRunes); err != nil {
		return PanelFooter{}, err
	}
	action := strings.TrimSpace(footer.Action)
	if action != "" {
		if utf8.RuneCountInString(action) > panelMaxActionRunes || !validPanelAction(action) {
			return PanelFooter{}, invalidArg("footer action must match [A-Za-z0-9._-]")
		}
	}
	return PanelFooter{Label: label, Action: action}, nil
}

func checkPanelText(value string, maxRunes int) *APIError {
	if strings.ContainsRune(value, 0) {
		return invalidArg("text must not contain NUL")
	}
	if utf8.RuneCountInString(value) > maxRunes {
		return invalidArg("text is too long")
	}
	return nil
}

func validPanelAction(action string) bool {
	if action == "" || utf8.RuneCountInString(action) > panelMaxActionRunes {
		return false
	}
	for _, r := range action {
		switch {
		case r >= 'a' && r <= 'z':
		case r >= 'A' && r <= 'Z':
		case r >= '0' && r <= '9':
		case r == '.' || r == '_' || r == '-':
		default:
			return false
		}
	}
	return true
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
