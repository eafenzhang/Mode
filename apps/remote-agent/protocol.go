package main

import (
	"encoding/json"
	"fmt"
	"runtime"
	"strings"
	"time"
	"unicode/utf8"
)

const (
	agentName        = "mode-agent"
	mcpServerName    = "mode-agent"
	mcpLatestVersion = "2025-11-25"
)

var (
	version   = "0.1.0"
	goVersion = runtime.Version()
)

// APIError is the only error shape crossing the HTTP/MCP boundary. Status is
// transport metadata and never serialized into the JSON body.
type APIError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
	Status  int    `json:"-"`
}

func (e *APIError) Error() string {
	return e.Code + ": " + e.Message
}

func apiError(code, message string, status int) *APIError {
	return &APIError{Code: code, Message: message, Status: status}
}

func invalidArg(message string) *APIError {
	return apiError("invalid_argument", message, 400)
}

func capabilityDisabled(name string) *APIError {
	return apiError("capability_disabled", name+" is disabled by the agent configuration", 403)
}

func pathNotAllowed(path string) *APIError {
	return apiError("path_not_allowed", "path is outside allowedRoots: "+path, 403)
}

type apiEnvelope struct {
	OK        bool        `json:"ok"`
	Result    interface{} `json:"result,omitempty"`
	Error     *APIError   `json:"error,omitempty"`
	RequestID string      `json:"requestId,omitempty"`
}

type Capabilities struct {
	Shell          bool `json:"shell"`
	Write          bool `json:"write"`
	Delete         bool `json:"delete"`
	DesktopControl bool `json:"desktopControl"`
	Database       bool `json:"database"`
	DatabaseWrite  bool `json:"databaseWrite"`
	Panel          bool `json:"panel"`
}

type ScreenSize struct {
	Width  int `json:"width"`
	Height int `json:"height"`
}

type DriveInfo struct {
	Path       string `json:"path"`
	Type       string `json:"type"`
	TotalBytes uint64 `json:"totalBytes"`
	FreeBytes  uint64 `json:"freeBytes"`
}

type InfoResult struct {
	Agent                string       `json:"agent"`
	Version              string       `json:"version"`
	GoVersion            string       `json:"goVersion"`
	Hostname             string       `json:"hostname"`
	User                 string       `json:"user"`
	OS                   string       `json:"os"`
	Arch                 string       `json:"arch"`
	WindowsVersion       string       `json:"windowsVersion"`
	WindowsBuild         uint32       `json:"windowsBuild"`
	SessionID            uint32       `json:"sessionId"`
	ActiveSessionID      uint32       `json:"activeSessionId"`
	DesktopAvailable     bool         `json:"desktopAvailable"`
	UptimeSeconds        int64        `json:"uptimeSeconds"`
	MemoryTotalBytes     uint64       `json:"memoryTotalBytes"`
	MemoryAvailableBytes uint64       `json:"memoryAvailableBytes"`
	Screen               ScreenSize   `json:"screen"`
	Drives               []DriveInfo  `json:"drives"`
	Capabilities         Capabilities `json:"capabilities"`
	StartedAt            string       `json:"startedAt"`
}

type ExecRequest struct {
	Command        string            `json:"command"`
	Shell          string            `json:"shell,omitempty"`
	Args           []string          `json:"args,omitempty"`
	Cwd            string            `json:"cwd,omitempty"`
	Env            map[string]string `json:"env,omitempty"`
	Stdin          string            `json:"stdin,omitempty"`
	Encoding       string            `json:"encoding,omitempty"`
	TimeoutMS      int               `json:"timeoutMs,omitempty"`
	MaxOutputBytes int               `json:"maxOutputBytes,omitempty"`
}

type ExecResult struct {
	JobID            string `json:"jobId"`
	Status           string `json:"status"`
	ExitCode         int    `json:"exitCode"`
	TimedOut         bool   `json:"timedOut"`
	Canceled         bool   `json:"canceled"`
	Stdout           string `json:"stdout"`
	Stderr           string `json:"stderr"`
	StdoutNextOffset int64  `json:"stdoutNextOffset"`
	StderrNextOffset int64  `json:"stderrNextOffset"`
	Truncated        bool   `json:"truncated"`
	DurationMS       int64  `json:"durationMs"`
}

type JobSummary struct {
	JobID      string `json:"jobId"`
	Status     string `json:"status"`
	ExitCode   int    `json:"exitCode"`
	CreatedAt  string `json:"createdAt"`
	StartedAt  string `json:"startedAt,omitempty"`
	EndedAt    string `json:"endedAt,omitempty"`
	DurationMS int64  `json:"durationMs"`
}

type JobListResult struct {
	Jobs []JobSummary `json:"jobs"`
}

type JobStatusResult struct {
	JobID            string `json:"jobId"`
	Status           string `json:"status"`
	ExitCode         int    `json:"exitCode"`
	TimedOut         bool   `json:"timedOut"`
	Canceled         bool   `json:"canceled"`
	CreatedAt        string `json:"createdAt"`
	StartedAt        string `json:"startedAt,omitempty"`
	EndedAt          string `json:"endedAt,omitempty"`
	DurationMS       int64  `json:"durationMs"`
	Stdout           string `json:"stdout"`
	StdoutNextOffset int64  `json:"stdoutNextOffset"`
	Stderr           string `json:"stderr"`
	StderrNextOffset int64  `json:"stderrNextOffset"`
	Truncated        bool   `json:"truncated"`
}

type JobStdinRequest struct {
	Data     string `json:"data"`
	Encoding string `json:"encoding,omitempty"`
	Close    bool   `json:"close,omitempty"`
}

type JobStdinResult struct {
	JobID  string `json:"jobId"`
	Bytes  int    `json:"bytes"`
	Closed bool   `json:"closed"`
}

type FSListRequest struct {
	Path       string `json:"path"`
	MaxEntries int    `json:"maxEntries,omitempty"`
}

type FSEntry struct {
	Name     string `json:"name"`
	Path     string `json:"path"`
	Dir      bool   `json:"dir"`
	Size     int64  `json:"size"`
	Modified string `json:"modified"`
	Mode     string `json:"mode"`
}

type FSListResult struct {
	Path      string    `json:"path"`
	Entries   []FSEntry `json:"entries"`
	Truncated bool      `json:"truncated"`
}

type FSReadRequest struct {
	Path     string `json:"path"`
	Offset   int64  `json:"offset,omitempty"`
	MaxBytes int    `json:"maxBytes,omitempty"`
	Encoding string `json:"encoding,omitempty"`
}

type FSReadResult struct {
	Path       string `json:"path"`
	Size       int64  `json:"size"`
	Offset     int64  `json:"offset"`
	NextOffset int64  `json:"nextOffset"`
	Encoding   string `json:"encoding"`
	Text       string `json:"text,omitempty"`
	Base64     string `json:"base64,omitempty"`
	Truncated  bool   `json:"truncated"`
}

type FSWriteRequest struct {
	Path           string `json:"path"`
	Text           string `json:"text,omitempty"`
	Base64         string `json:"base64,omitempty"`
	Append         bool   `json:"append,omitempty"`
	CreateDirs     bool   `json:"createDirs,omitempty"`
	ExpectedSHA256 string `json:"expectedSha256,omitempty"`
}

type FSWriteResult struct {
	Path   string `json:"path"`
	Bytes  int64  `json:"bytes"`
	SHA256 string `json:"sha256"`
}

type FSStatResult struct {
	Path     string `json:"path"`
	Name     string `json:"name"`
	Dir      bool   `json:"dir"`
	Size     int64  `json:"size"`
	Modified string `json:"modified"`
	Mode     string `json:"mode"`
}

type FSRemoveRequest struct {
	Path      string `json:"path"`
	Recursive bool   `json:"recursive,omitempty"`
}

type FSRemoveResult struct {
	Path    string `json:"path"`
	Removed bool   `json:"removed"`
}

type ProcessInfo struct {
	PID       uint32 `json:"pid"`
	Name      string `json:"name"`
	Threads   uint32 `json:"threads"`
	ParentPID uint32 `json:"parentPid"`
}

type ProcessListResult struct {
	Processes []ProcessInfo `json:"processes"`
}

type ProcessKillRequest struct {
	PID   uint32 `json:"pid"`
	Force bool   `json:"force,omitempty"`
}

type ProcessKillResult struct {
	PID    uint32 `json:"pid"`
	Killed bool   `json:"killed"`
}

type ScreenRequest struct {
	MaxWidth  int `json:"maxWidth,omitempty"`
	MaxHeight int `json:"maxHeight,omitempty"`
}

type ScreenResult struct {
	Width     int    `json:"width"`
	Height    int    `json:"height"`
	Format    string `json:"format"`
	PNGBase64 string `json:"pngBase64"`
	Bytes     int    `json:"bytes"`
}

type MouseRequest struct {
	Action     string `json:"action"`
	X          int    `json:"x,omitempty"`
	Y          int    `json:"y,omitempty"`
	X1         int    `json:"x1,omitempty"`
	Y1         int    `json:"y1,omitempty"`
	X2         int    `json:"x2,omitempty"`
	Y2         int    `json:"y2,omitempty"`
	Button     string `json:"button,omitempty"`
	Delta      int    `json:"delta,omitempty"`
	DurationMS int    `json:"durationMs,omitempty"`
}

type MouseResult struct {
	Action string `json:"action"`
	X      int    `json:"x"`
	Y      int    `json:"y"`
}

type KeyboardRequest struct {
	Action string `json:"action,omitempty"`
	Text   string `json:"text,omitempty"`
	Key    string `json:"key,omitempty"`
}

type KeyboardResult struct {
	Action     string `json:"action"`
	Characters int    `json:"characters"`
}

type RectInfo struct {
	Left   int32 `json:"left"`
	Top    int32 `json:"top"`
	Right  int32 `json:"right"`
	Bottom int32 `json:"bottom"`
}

type WindowInfo struct {
	HWND      string   `json:"hwnd"`
	PID       uint32   `json:"pid"`
	Title     string   `json:"title"`
	Minimized bool     `json:"minimized"`
	Rect      RectInfo `json:"rect"`
}

type WindowListResult struct {
	Windows []WindowInfo `json:"windows"`
}

type WindowFocusRequest struct {
	HWND string `json:"hwnd"`
}

type WindowFocusResult struct {
	HWND    string `json:"hwnd"`
	Focused bool   `json:"focused"`
}

type DBDriversResult struct {
	Drivers []string `json:"drivers"`
}

type DBQueryRequest struct {
	DSN        string `json:"dsn,omitempty"`
	Connection string `json:"connection,omitempty"`
	SQL        string `json:"sql"`
	MaxRows    int    `json:"maxRows,omitempty"`
	TimeoutMS  int    `json:"timeoutMs,omitempty"`
	ReadOnly   *bool  `json:"readOnly,omitempty"`
}

type DBColumn struct {
	Name   string `json:"name"`
	Type   int16  `json:"type"`
	Binary bool   `json:"binary"`
}

type DBQueryResult struct {
	Columns   []DBColumn      `json:"columns"`
	Rows      [][]interface{} `json:"rows"`
	RowCount  int             `json:"rowCount"`
	Truncated bool            `json:"truncated"`
}

type PairRequest struct {
	Code  string `json:"code"`
	Label string `json:"label,omitempty"`
}

type PairResult struct {
	Token     string   `json:"token"`
	AgentID   string   `json:"agentId"`
	Name      string   `json:"name"`
	Hostname  string   `json:"hostname"`
	Version   string   `json:"version"`
	Platform  string   `json:"platform"`
	Port      int      `json:"port"`
	Addresses []string `json:"addresses"`
}

type PairCodeResult struct {
	Code      string   `json:"code"`
	ExpiresAt int64    `json:"expiresAt"`
	Port      int      `json:"port"`
	Name      string   `json:"name"`
	Addresses []string `json:"addresses"`
}

type mcpRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

type mcpError struct {
	Code    int         `json:"code"`
	Message string      `json:"message"`
	Data    interface{} `json:"data,omitempty"`
}

type mcpResponse struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Result  interface{}     `json:"result,omitempty"`
	Error   *mcpError       `json:"error,omitempty"`
}

type mcpContent struct {
	Type     string `json:"type"`
	Text     string `json:"text,omitempty"`
	Data     string `json:"data,omitempty"`
	MimeType string `json:"mimeType,omitempty"`
}

type mcpToolResult struct {
	Content []mcpContent `json:"content"`
	IsError bool         `json:"isError,omitempty"`
}

type mcpTool struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"inputSchema"`
}

func nowRFC3339(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.UTC().Format(time.RFC3339Nano)
}

func defaultString(value, fallback string) string {
	if strings.TrimSpace(value) == "" {
		return fallback
	}
	return value
}

func sanitizeUTF8(data []byte) string {
	if utf8.Valid(data) {
		return string(data)
	}
	var b strings.Builder
	for len(data) > 0 {
		r, size := utf8.DecodeRune(data)
		if r == utf8.RuneError && size == 1 {
			b.WriteRune(utf8.RuneError)
			data = data[1:]
			continue
		}
		b.WriteRune(r)
		data = data[size:]
	}
	return b.String()
}

func textJSON(value interface{}) (string, *APIError) {
	data, err := json.Marshal(value)
	if err != nil {
		return "", apiError("internal_error", "failed to encode result", 500)
	}
	return fmt.Sprintf("%s", data), nil
}
