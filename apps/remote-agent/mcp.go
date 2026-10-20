package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io/ioutil"
	"net/http"
	"strconv"
	"strings"
)

func (s *agentServer) handleMCP(w http.ResponseWriter, r *http.Request, requestID string) {
	switch r.Method {
	case "POST":
	case "GET", "DELETE":
		w.Header().Set("Allow", "POST, OPTIONS")
		w.WriteHeader(http.StatusMethodNotAllowed)
		return
	default:
		s.methodNotAllowed(w, requestID, "POST, OPTIONS")
		return
	}
	body, err := ioutil.ReadAll(http.MaxBytesReader(w, r.Body, 4<<20))
	if err != nil {
		s.writeMCP(w, mcpResponse{JSONRPC: "2.0", ID: json.RawMessage("null"), Error: &mcpError{Code: -32700, Message: "parse error"}})
		return
	}
	trimmed := bytes.TrimSpace(body)
	if len(trimmed) == 0 {
		s.writeMCP(w, mcpResponse{JSONRPC: "2.0", ID: json.RawMessage("null"), Error: &mcpError{Code: -32700, Message: "empty request"}})
		return
	}
	if trimmed[0] == '[' {
		var rawMessages []json.RawMessage
		if err := json.Unmarshal(trimmed, &rawMessages); err != nil {
			s.writeMCP(w, mcpResponse{JSONRPC: "2.0", ID: json.RawMessage("null"), Error: &mcpError{Code: -32700, Message: "parse error"}})
			return
		}
		responses := make([]mcpResponse, 0, len(rawMessages))
		for _, raw := range rawMessages {
			response, hasResponse := s.handleMCPMessage(r.Context(), raw)
			if hasResponse {
				responses = append(responses, response)
			}
		}
		if len(responses) == 0 {
			w.WriteHeader(http.StatusAccepted)
			return
		}
		s.writeMCP(w, responses)
		return
	}
	response, hasResponse := s.handleMCPMessage(r.Context(), trimmed)
	if !hasResponse {
		w.WriteHeader(http.StatusAccepted)
		return
	}
	s.writeMCP(w, response)
}

func (s *agentServer) writeMCP(w http.ResponseWriter, response interface{}) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(http.StatusOK)
	_ = json.NewEncoder(w).Encode(response)
}

func (s *agentServer) handleMCPMessage(ctx context.Context, raw json.RawMessage) (mcpResponse, bool) {
	var request mcpRequest
	if err := json.Unmarshal(raw, &request); err != nil {
		return mcpResponse{JSONRPC: "2.0", Error: &mcpError{Code: -32700, Message: "parse error"}}, true
	}
	hasID := len(request.ID) > 0
	if request.JSONRPC != "" && request.JSONRPC != "2.0" {
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Error: &mcpError{Code: -32600, Message: "invalid JSON-RPC version"}}, hasID
	}
	if request.Method == "" {
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Error: &mcpError{Code: -32600, Message: "method is required"}}, hasID
	}
	switch request.Method {
	case "notifications/initialized", "notifications/cancelled":
		return mcpResponse{}, false
	case "initialize":
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Result: s.mcpInitializeResult(request.Params)}, hasID
	case "ping":
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Result: map[string]interface{}{}}, hasID
	case "tools/list":
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Result: map[string]interface{}{"tools": mcpTools()}}, hasID
	case "tools/call":
		var params struct {
			Name      string                 `json:"name"`
			Arguments map[string]interface{} `json:"arguments"`
		}
		if err := json.Unmarshal(request.Params, &params); err != nil || strings.TrimSpace(params.Name) == "" {
			return mcpResponse{JSONRPC: "2.0", ID: request.ID, Error: &mcpError{Code: -32602, Message: "tools/call requires name and arguments"}}, hasID
		}
		if params.Arguments == nil {
			params.Arguments = map[string]interface{}{}
		}
		result, apiErr := s.callMCPTool(ctx, params.Name, params.Arguments)
		if apiErr != nil {
			result = mcpErrorResult(apiErr)
		}
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Result: result}, hasID
	default:
		return mcpResponse{JSONRPC: "2.0", ID: request.ID, Error: &mcpError{Code: -32601, Message: "method not found: " + request.Method}}, hasID
	}
}

func (s *agentServer) mcpInitializeResult(params json.RawMessage) map[string]interface{} {
	var input struct {
		ProtocolVersion string `json:"protocolVersion"`
	}
	_ = json.Unmarshal(params, &input)
	return map[string]interface{}{
		"protocolVersion": negotiateMCPVersion(input.ProtocolVersion),
		"capabilities":    map[string]interface{}{"tools": map[string]interface{}{}},
		"serverInfo": map[string]string{
			"name":    mcpServerName,
			"version": version,
		},
		"instructions": "This server controls a remote Windows machine. Commands run with the agent user's privileges. " +
			"Desktop input requires allowDesktopControl; filesystem writes and deletes are disabled unless configured. " +
			"Capture the screen before injecting mouse or keyboard input, and use named DSNs for database access.",
	}
}

func negotiateMCPVersion(requested string) string {
	for _, candidate := range []string{"2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05", "2024-10-07"} {
		if requested == candidate {
			return candidate
		}
	}
	return mcpLatestVersion
}

func mcpErrorResult(apiErr *APIError) mcpToolResult {
	return mcpToolResult{
		Content: []mcpContent{{Type: "text", Text: apiErr.Code + ": " + apiErr.Message}},
		IsError: true,
	}
}

func textMCPResult(value interface{}) (mcpToolResult, *APIError) {
	text, apiErr := textJSON(value)
	if apiErr != nil {
		return mcpToolResult{}, apiErr
	}
	return mcpToolResult{Content: []mcpContent{{Type: "text", Text: text}}}, nil
}

func (s *agentServer) callMCPTool(ctx context.Context, name string, args map[string]interface{}) (mcpToolResult, *APIError) {
	switch name {
	case "remote_info":
		info, apiErr := s.infoResult()
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(info)
	case "remote_exec":
		req, apiErr := execRequestFromArgs(args)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		result, apiErr := s.execSync(ctx, req)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_job_start":
		req, apiErr := execRequestFromArgs(args)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		result, apiErr := s.startBackgroundJob(req)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_job_status":
		jobID, ok := argString(args, "jobId")
		if !ok || jobID == "" {
			return mcpToolResult{}, invalidArg("jobId is required")
		}
		job, apiErr := s.getJob(jobID)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		encoding, _ := argString(args, "encoding")
		result := job.snapshot(int64(argIntDefault(args, "stdoutOffset", 0)), int64(argIntDefault(args, "stderrOffset", 0)), encoding)
		return textMCPResult(result)
	case "remote_job_write":
		jobID, ok := argString(args, "jobId")
		if !ok || jobID == "" {
			return mcpToolResult{}, invalidArg("jobId is required")
		}
		job, apiErr := s.getJob(jobID)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		data, ok := argString(args, "data")
		if !ok {
			return mcpToolResult{}, invalidArg("data must be a string")
		}
		var payload []byte
		encoding, _ := argString(args, "encoding")
		if strings.EqualFold(encoding, "base64") {
			decoded, err := decodeBase64(data)
			if err != nil {
				return mcpToolResult{}, invalidArg("data is not valid base64")
			}
			payload = decoded
		} else {
			payload = []byte(data)
		}
		closeAfter, _ := argBool(args, "close", false)
		n, err := job.writeStdin(payload, closeAfter)
		if err != nil {
			return mcpToolResult{}, apiError("job_stdin_failed", err.Error(), 409)
		}
		return textMCPResult(JobStdinResult{JobID: job.ID, Bytes: n, Closed: closeAfter})
	case "remote_job_cancel":
		jobID, ok := argString(args, "jobId")
		if !ok || jobID == "" {
			return mcpToolResult{}, invalidArg("jobId is required")
		}
		job, apiErr := s.getJob(jobID)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		job.terminateAndWait("canceled")
		return textMCPResult(job.snapshot(0, 0, ""))
	case "remote_job_list":
		return textMCPResult(s.jobList())
	case "remote_fs_list":
		path, ok := argString(args, "path")
		if !ok || path == "" {
			return mcpToolResult{}, invalidArg("path is required")
		}
		result, apiErr := s.fsList(FSListRequest{Path: path, MaxEntries: argIntDefault(args, "maxEntries", 0)})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_fs_read":
		path, ok := argString(args, "path")
		if !ok || path == "" {
			return mcpToolResult{}, invalidArg("path is required")
		}
		encoding, _ := argString(args, "encoding")
		result, apiErr := s.fsRead(FSReadRequest{
			Path:     path,
			Offset:   int64(argIntDefault(args, "offset", 0)),
			MaxBytes: argIntDefault(args, "maxBytes", 0),
			Encoding: encoding,
		})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_fs_write":
		path, ok := argString(args, "path")
		if !ok || path == "" {
			return mcpToolResult{}, invalidArg("path is required")
		}
		text, _ := argString(args, "text")
		base64Data, _ := argString(args, "base64")
		expected, _ := argString(args, "expectedSha256")
		appendData, _ := argBool(args, "append", false)
		createDirs, _ := argBool(args, "createDirs", false)
		result, apiErr := s.fsWrite(FSWriteRequest{
			Path:           path,
			Text:           text,
			Base64:         base64Data,
			Append:         appendData,
			CreateDirs:     createDirs,
			ExpectedSHA256: expected,
		})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_fs_stat":
		path, ok := argString(args, "path")
		if !ok || path == "" {
			return mcpToolResult{}, invalidArg("path is required")
		}
		result, apiErr := s.fsStat(path)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_fs_remove":
		path, ok := argString(args, "path")
		if !ok || path == "" {
			return mcpToolResult{}, invalidArg("path is required")
		}
		recursive, _ := argBool(args, "recursive", false)
		result, apiErr := s.fsRemove(FSRemoveRequest{Path: path, Recursive: recursive})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_process_list":
		result, apiErr := processList()
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_process_kill":
		if !s.cfg.AllowShell {
			return mcpToolResult{}, capabilityDisabled("shell")
		}
		pid, ok := argUint32(args, "pid")
		if !ok || pid == 0 {
			return mcpToolResult{}, invalidArg("pid is required")
		}
		result, apiErr := processKill(ProcessKillRequest{PID: pid})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_screen_capture":
		if !s.cfg.AllowDesktopControl {
			return mcpToolResult{}, capabilityDisabled("desktop control")
		}
		select {
		case s.desktopSem <- struct{}{}:
		default:
			return mcpToolResult{}, apiError("busy", "another desktop operation is running", 429)
		}
		defer func() { <-s.desktopSem }()
		result, apiErr := captureScreen(ScreenRequest{
			MaxWidth:  argIntDefault(args, "maxWidth", 1280),
			MaxHeight: argIntDefault(args, "maxHeight", 960),
		})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return mcpToolResult{Content: []mcpContent{
			{Type: "image", Data: result.PNGBase64, MimeType: "image/png"},
			{Type: "text", Text: fmt.Sprintf("captured %dx%d png (%d bytes)", result.Width, result.Height, result.Bytes)},
		}}, nil
	case "remote_mouse":
		if !s.cfg.AllowDesktopControl {
			return mcpToolResult{}, capabilityDisabled("desktop control")
		}
		req := MouseRequest{
			Action:     argStringDefault(args, "action", "click"),
			X:          argIntDefault(args, "x", 0),
			Y:          argIntDefault(args, "y", 0),
			X1:         argIntDefault(args, "x1", 0),
			Y1:         argIntDefault(args, "y1", 0),
			X2:         argIntDefault(args, "x2", 0),
			Y2:         argIntDefault(args, "y2", 0),
			Button:     argStringDefault(args, "button", "left"),
			Delta:      argIntDefault(args, "delta", 0),
			DurationMS: argIntDefault(args, "durationMs", 0),
		}
		select {
		case s.desktopSem <- struct{}{}:
		default:
			return mcpToolResult{}, apiError("busy", "another desktop operation is running", 429)
		}
		defer func() { <-s.desktopSem }()
		result, apiErr := injectMouse(req)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_keyboard":
		if !s.cfg.AllowDesktopControl {
			return mcpToolResult{}, capabilityDisabled("desktop control")
		}
		req := KeyboardRequest{
			Action: argStringDefault(args, "action", ""),
			Text:   argStringDefault(args, "text", ""),
			Key:    argStringDefault(args, "key", ""),
		}
		select {
		case s.desktopSem <- struct{}{}:
		default:
			return mcpToolResult{}, apiError("busy", "another desktop operation is running", 429)
		}
		defer func() { <-s.desktopSem }()
		result, apiErr := injectKeyboard(req)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_windows_list":
		if !s.cfg.AllowDesktopControl {
			return mcpToolResult{}, capabilityDisabled("desktop control")
		}
		result, apiErr := listWindows()
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_window_focus":
		if !s.cfg.AllowDesktopControl {
			return mcpToolResult{}, capabilityDisabled("desktop control")
		}
		hwnd, ok := argString(args, "hwnd")
		if !ok || hwnd == "" {
			return mcpToolResult{}, invalidArg("hwnd is required")
		}
		result, apiErr := focusWindow(WindowFocusRequest{HWND: hwnd})
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_db_drivers":
		if !s.cfg.AllowDB {
			return mcpToolResult{}, capabilityDisabled("database")
		}
		select {
		case s.dbSem <- struct{}{}:
		default:
			return mcpToolResult{}, apiError("busy", "too many concurrent database operations", 429)
		}
		defer func() { <-s.dbSem }()
		result, apiErr := listDBDrivers()
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	case "remote_db_query":
		if !s.cfg.AllowDB {
			return mcpToolResult{}, capabilityDisabled("database")
		}
		connection, _ := argString(args, "connection")
		dsn, _ := argString(args, "dsn")
		sqlText, ok := argString(args, "sql")
		if !ok || strings.TrimSpace(sqlText) == "" {
			return mcpToolResult{}, invalidArg("sql is required")
		}
		readOnly, _ := argBool(args, "readOnly", true)
		req := DBQueryRequest{
			DSN:        dsn,
			Connection: connection,
			SQL:        sqlText,
			MaxRows:    argIntDefault(args, "maxRows", 200),
			TimeoutMS:  argIntDefault(args, "timeoutMs", 30000),
			ReadOnly:   &readOnly,
		}
		select {
		case s.dbSem <- struct{}{}:
		default:
			return mcpToolResult{}, apiError("busy", "too many concurrent database operations", 429)
		}
		defer func() { <-s.dbSem }()
		result, apiErr := queryDB(req, s.cfg)
		if apiErr != nil {
			return mcpToolResult{}, apiErr
		}
		return textMCPResult(result)
	default:
		return mcpToolResult{}, apiError("tool_not_found", "unknown tool: "+name, 404)
	}
}

func (s *agentServer) infoResult() (*InfoResult, *APIError) {
	info, apiErr := platformInfo()
	if apiErr != nil {
		return nil, apiErr
	}
	info.Agent = agentName
	info.Version = version
	info.GoVersion = goVersion
	info.Capabilities = s.capabilities()
	info.StartedAt = nowRFC3339(s.started)
	return info, nil
}

func execRequestFromArgs(args map[string]interface{}) (ExecRequest, *APIError) {
	command, ok := argString(args, "command")
	if !ok || strings.TrimSpace(command) == "" {
		return ExecRequest{}, invalidArg("command is required")
	}
	req := ExecRequest{
		Command:        command,
		Shell:          argStringDefault(args, "shell", "cmd"),
		Cwd:            argStringDefault(args, "cwd", ""),
		Stdin:          argStringDefault(args, "stdin", ""),
		Encoding:       argStringDefault(args, "encoding", "auto"),
		TimeoutMS:      argIntDefault(args, "timeoutMs", 0),
		MaxOutputBytes: argIntDefault(args, "maxOutputBytes", 0),
	}
	if commandArgs, ok := argStringSlice(args, "args"); ok {
		req.Args = commandArgs
	}
	if env, ok := argStringMap(args, "env"); ok {
		req.Env = env
	}
	return req, nil
}

func argString(args map[string]interface{}, key string) (string, bool) {
	value, exists := args[key]
	if !exists || value == nil {
		return "", true
	}
	text, ok := value.(string)
	return text, ok
}

func argStringDefault(args map[string]interface{}, key, fallback string) string {
	value, ok := argString(args, key)
	if !ok || value == "" {
		return fallback
	}
	return value
}

func argIntDefault(args map[string]interface{}, key string, fallback int) int {
	value, exists := args[key]
	if !exists || value == nil {
		return fallback
	}
	switch typed := value.(type) {
	case float64:
		return int(typed)
	case int:
		return typed
	case json.Number:
		parsed, err := typed.Int64()
		if err == nil {
			return int(parsed)
		}
	}
	return fallback
}

func argBool(args map[string]interface{}, key string, fallback bool) (bool, bool) {
	value, exists := args[key]
	if !exists || value == nil {
		return fallback, true
	}
	typed, ok := value.(bool)
	if !ok {
		return fallback, false
	}
	return typed, true
}

func argUint32(args map[string]interface{}, key string) (uint32, bool) {
	value, exists := args[key]
	if !exists || value == nil {
		return 0, true
	}
	switch typed := value.(type) {
	case float64:
		if typed < 0 || typed > 4294967295 {
			return 0, false
		}
		return uint32(typed), true
	case string:
		parsed, err := parseUint32String(typed)
		if err != nil {
			return 0, false
		}
		return parsed, true
	}
	return 0, false
}

func argStringSlice(args map[string]interface{}, key string) ([]string, bool) {
	value, exists := args[key]
	if !exists || value == nil {
		return nil, true
	}
	raw, ok := value.([]interface{})
	if !ok {
		return nil, false
	}
	result := make([]string, 0, len(raw))
	for _, item := range raw {
		text, ok := item.(string)
		if !ok {
			return nil, false
		}
		result = append(result, text)
	}
	return result, true
}

func argStringMap(args map[string]interface{}, key string) (map[string]string, bool) {
	value, exists := args[key]
	if !exists || value == nil {
		return nil, true
	}
	raw, ok := value.(map[string]interface{})
	if !ok {
		return nil, false
	}
	result := make(map[string]string, len(raw))
	for name, item := range raw {
		text, ok := item.(string)
		if !ok {
			return nil, false
		}
		result[name] = text
	}
	return result, true
}

func mcpTools() []mcpTool {
	return []mcpTool{
		{
			Name:        "remote_info",
			Description: "Return the remote Windows host identity, version, session, screen size, drives, and enabled capabilities.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`),
		},
		{
			Name:        "remote_exec",
			Description: "Run a command on the remote Windows host and wait for completion. Returns exitCode, stdout, stderr, timeout/cancel flags, and output offsets.",
			InputSchema: execSchema(),
		},
		{
			Name:        "remote_job_start",
			Description: "Start a command as a background job and return its jobId. Poll with remote_job_status; write stdin with remote_job_write; stop with remote_job_cancel.",
			InputSchema: execSchema(),
		},
		{
			Name:        "remote_job_status",
			Description: "Read incremental stdout/stderr and lifecycle state for a background job.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"jobId":{"type":"string"},"stdoutOffset":{"type":"integer","minimum":0},"stderrOffset":{"type":"integer","minimum":0},"encoding":{"type":"string","enum":["auto","utf8","oem","ansi"]}},"required":["jobId"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_job_write",
			Description: "Write text or base64 data to a background job's stdin. Set close=true to send EOF.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"jobId":{"type":"string"},"data":{"type":"string"},"encoding":{"type":"string","enum":["text","base64"]},"close":{"type":"boolean"}},"required":["jobId","data"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_job_cancel",
			Description: "Terminate a background job and its process tree.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"jobId":{"type":"string"}},"required":["jobId"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_job_list",
			Description: "List retained background jobs without stdout/stderr content.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`),
		},
		{
			Name:        "remote_fs_list",
			Description: "List a remote directory. Paths are subject to allowedRoots when configured.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"},"maxEntries":{"type":"integer","minimum":1,"maximum":10000}},"required":["path"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_fs_read",
			Description: "Read a remote file. Returns text or base64 depending on encoding/autodetection. Use offset and maxBytes to page.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"maxBytes":{"type":"integer","minimum":1,"maximum":16777216},"encoding":{"type":"string","enum":["auto","utf8","oem","ansi","binary"]}},"required":["path"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_fs_write",
			Description: "Write a remote file. Requires allowWrite. Provide exactly one of text or base64. Non-append writes use a temporary file then replace.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"base64":{"type":"string"},"append":{"type":"boolean"},"createDirs":{"type":"boolean"},"expectedSha256":{"type":"string"}},"required":["path"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_fs_stat",
			Description: "Return metadata for a remote file or directory.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_fs_remove",
			Description: "Delete a remote file or directory. Requires allowDelete; recursive must be explicitly set for non-empty directories.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"}},"required":["path"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_process_list",
			Description: "List running processes with pid, name, thread count, and parent pid.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`),
		},
		{
			Name:        "remote_process_kill",
			Description: "Terminate a process by pid. System processes and the agent itself are refused.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"pid":{"type":"integer","minimum":1}},"required":["pid"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_screen_capture",
			Description: "Capture the remote virtual desktop as PNG. Call this before mouse/keyboard input to locate targets. Requires allowDesktopControl.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"maxWidth":{"type":"integer","minimum":64,"maximum":8192},"maxHeight":{"type":"integer","minimum":64,"maximum":8192}},"additionalProperties":false}`),
		},
		{
			Name:        "remote_mouse",
			Description: "Inject mouse input using absolute screenshot coordinates. Actions: move, click, down, up, scroll, drag. Requires allowDesktopControl.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"action":{"type":"string","enum":["move","click","down","up","scroll","drag"]},"x":{"type":"integer"},"y":{"type":"integer"},"x1":{"type":"integer"},"y1":{"type":"integer"},"x2":{"type":"integer"},"y2":{"type":"integer"},"button":{"type":"string","enum":["left","right","middle"]},"delta":{"type":"integer"},"durationMs":{"type":"integer","minimum":0,"maximum":10000}},"required":["action"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_keyboard",
			Description: "Type Unicode text or press a key combination such as CTRL+S. Requires allowDesktopControl. Secure attention sequences are not supported.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"action":{"type":"string","enum":["type","key"]},"text":{"type":"string"},"key":{"type":"string"}},"additionalProperties":false}`),
		},
		{
			Name:        "remote_windows_list",
			Description: "List visible top-level windows with hwnd, pid, title, minimized state, and rectangle.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`),
		},
		{
			Name:        "remote_window_focus",
			Description: "Restore and focus a top-level window by hwnd from remote_windows_list.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"hwnd":{"type":"string"}},"required":["hwnd"],"additionalProperties":false}`),
		},
		{
			Name:        "remote_db_drivers",
			Description: "List ODBC drivers visible to this agent process. A 32-bit agent sees 32-bit drivers only.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{},"additionalProperties":false}`),
		},
		{
			Name:        "remote_db_query",
			Description: "Run SQL through ODBC. Prefer a named dsn configured on the agent; readOnly defaults to true and permits only SELECT/WITH/SHOW/DESCRIBE/EXPLAIN/PRAGMA.",
			InputSchema: json.RawMessage(`{"type":"object","properties":{"dsn":{"type":"string"},"connection":{"type":"string"},"sql":{"type":"string"},"maxRows":{"type":"integer","minimum":1,"maximum":5000},"timeoutMs":{"type":"integer","minimum":1,"maximum":600000},"readOnly":{"type":"boolean"}},"required":["sql"],"additionalProperties":false}`),
		},
	}
}

func execSchema() json.RawMessage {
	return json.RawMessage(`{"type":"object","properties":{"command":{"type":"string"},"shell":{"type":"string","enum":["cmd","powershell","none"]},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"},"env":{"type":"object","additionalProperties":{"type":"string"}},"stdin":{"type":"string"},"encoding":{"type":"string","enum":["auto","utf8","oem","ansi"]},"timeoutMs":{"type":"integer","minimum":0,"maximum":86400000},"maxOutputBytes":{"type":"integer","minimum":4096,"maximum":4194304}},"required":["command"],"additionalProperties":false}`)
}

func parseUint32String(value string) (uint32, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0, invalidArg("empty number")
	}
	base := 10
	if strings.HasPrefix(strings.ToLower(value), "0x") {
		value = value[2:]
		base = 16
	}
	parsed, err := strconv.ParseUint(value, base, 32)
	if err != nil {
		return 0, invalidArg("invalid number: " + value)
	}
	return uint32(parsed), nil
}
