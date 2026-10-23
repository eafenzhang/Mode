package main

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	maxJSONBody       = 1 << 20
	maxFSWriteBody    = 16 << 20
	maxResponseMemory = 16 << 20
	maxPanelBody      = panelMaxBase64 + panelMaxTextBytes + (64 << 10)
)

type agentServer struct {
	cfg     *Config
	audit   *auditLogger
	logger  *log.Logger
	started time.Time

	jobs   map[string]*Job
	jobsMu sync.Mutex

	execSem    chan struct{}
	desktopSem chan struct{}
	dbSem      chan struct{}

	allowCIDRs   []*net.IPNet
	pairing      *pairingState
	panel        *panelController
	stopGC       chan struct{}
	shutdownCh   chan struct{}
	shutdownOnce sync.Once
	closeOnce    sync.Once
}

func newAgentServer(cfg *Config) (*agentServer, error) {
	cidrs, err := parseCIDRs(cfg.AllowCIDRs)
	if err != nil {
		return nil, err
	}
	audit, auditErr := newAuditLogger(cfg.DataDir)
	logger := log.New(os.Stderr, "mode-agent: ", log.LstdFlags)
	if auditErr != nil {
		logger.Printf("audit log disabled: %v", auditErr)
	}
	server := &agentServer{
		cfg:        cfg,
		audit:      audit,
		logger:     logger,
		started:    time.Now(),
		jobs:       map[string]*Job{},
		execSem:    make(chan struct{}, 4),
		desktopSem: make(chan struct{}, 1),
		dbSem:      make(chan struct{}, 4),
		allowCIDRs: cidrs,
		pairing:    newPairingState(),
		panel:      newPanelController(),
		stopGC:     make(chan struct{}),
		shutdownCh: make(chan struct{}),
	}
	go server.jobGC()
	return server, nil
}

func (s *agentServer) requestShutdown() {
	s.shutdownOnce.Do(func() {
		// 守护进程靠这个文件区分“用户主动退出”和“进程被杀”；没有它会在退出后重启。
		writeStopFile(s.cfg)
		close(s.shutdownCh)
	})
}

func (s *agentServer) close() {
	s.closeOnce.Do(func() {
		close(s.stopGC)
		s.jobsMu.Lock()
		for _, job := range s.jobs {
			job.terminate("canceled")
		}
		s.jobsMu.Unlock()
		s.audit.close()
	})
}

func (s *agentServer) jobGC() {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			s.gcJobs()
		case <-s.stopGC:
			return
		}
	}
}

type statusWriter struct {
	http.ResponseWriter
	status int
}

func (w *statusWriter) WriteHeader(status int) {
	if w.status != 0 {
		return
	}
	w.status = status
	w.ResponseWriter.WriteHeader(status)
}

func (w *statusWriter) Write(data []byte) (int, error) {
	if w.status == 0 {
		w.status = 200
	}
	return w.ResponseWriter.Write(data)
}

func (w *statusWriter) currentStatus() int {
	if w.status == 0 {
		return 200
	}
	return w.status
}

func newRequestID() string {
	raw := make([]byte, 8)
	if _, err := rand.Read(raw); err != nil {
		return fmt.Sprintf("%x", time.Now().UnixNano())
	}
	return hex.EncodeToString(raw)
}

func (s *agentServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	requestID := newRequestID()
	sw := &statusWriter{ResponseWriter: w}
	sw.Header().Set("X-Request-Id", requestID)
	sw.Header().Set("Cache-Control", "no-store")
	sw.Header().Set("X-Content-Type-Options", "nosniff")
	defer func() {
		s.audit.write(auditEntry{
			RequestID:  requestID,
			Remote:     remoteHost(r),
			Method:     r.Method,
			Path:       r.URL.Path,
			Status:     sw.currentStatus(),
			DurationMS: time.Since(start).Nanoseconds() / int64(time.Millisecond),
			ErrorCode:  sw.Header().Get("X-Mode-Error-Code"),
		})
	}()
	defer func() {
		if recovered := recover(); recovered != nil {
			s.writeError(sw, requestID, apiError("internal_error", "internal error", 500))
		}
	}()

	path := r.URL.Path
	if path == "/v1/pair" {
		setPairCORS(sw)
	}
	if r.Method == "OPTIONS" {
		if path == "/v1/pair" {
			sw.Header().Set("Allow", "POST, OPTIONS")
		} else {
			sw.Header().Set("Allow", "GET, POST, OPTIONS")
		}
		sw.WriteHeader(http.StatusNoContent)
		return
	}
	if path == "/v1/pair" {
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST, OPTIONS")
			return
		}
		s.handlePair(sw, r, requestID)
		return
	}
	if path == "/health" {
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.writeResult(sw, http.StatusOK, requestID, map[string]interface{}{
			"agent":   agentName,
			"version": version,
			"time":    nowRFC3339(time.Now()),
		})
		return
	}
	if !s.tokenMatches(extractToken(r)) {
		sw.Header().Set("WWW-Authenticate", `Bearer realm="mode-agent"`)
		s.writeError(sw, requestID, apiError("unauthorized", "missing or invalid token", 401))
		return
	}
	if !s.ipAllowed(r) {
		s.writeError(sw, requestID, apiError("forbidden", "source address is not allowed", 403))
		return
	}

	switch {
	case path == "/v1/info":
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.handleInfo(sw, requestID)
	case path == "/v1/pair-code":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handlePairCode(sw, requestID)
	case path == "/v1/shutdown":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.requestShutdown()
		s.writeResult(sw, http.StatusOK, requestID, map[string]interface{}{"shuttingDown": true})
	case path == "/v1/panel":
		s.handlePanel(sw, r, requestID)
	case path == "/v1/panel/actions":
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.handlePanelActions(sw, r, requestID)
	case path == "/v1/exec":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleExec(sw, r, requestID)
	case path == "/v1/jobs":
		switch r.Method {
		case "GET":
			s.writeResult(sw, http.StatusOK, requestID, s.jobList())
		case "POST":
			s.handleJobStart(sw, r, requestID)
		default:
			s.methodNotAllowed(sw, requestID, "GET, POST")
		}
	case strings.HasPrefix(path, "/v1/jobs/"):
		s.handleJobSubpath(sw, r, requestID)
	case path == "/v1/fs/list":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleFSList(sw, r, requestID)
	case path == "/v1/fs/read":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleFSRead(sw, r, requestID)
	case path == "/v1/fs/write":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleFSWrite(sw, r, requestID)
	case path == "/v1/fs/stat":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleFSStat(sw, r, requestID)
	case path == "/v1/fs/remove":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleFSRemove(sw, r, requestID)
	case path == "/v1/processes":
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.handleProcessList(sw, requestID)
	case path == "/v1/process/kill":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleProcessKill(sw, r, requestID)
	case path == "/v1/screen/capture":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleScreen(sw, r, requestID)
	case path == "/v1/input/mouse":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleMouse(sw, r, requestID)
	case path == "/v1/input/key":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleKeyboard(sw, r, requestID)
	case path == "/v1/windows":
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.handleWindowList(sw, requestID)
	case path == "/v1/window/focus":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleWindowFocus(sw, r, requestID)
	case path == "/v1/db/drivers":
		if r.Method != "GET" {
			s.methodNotAllowed(sw, requestID, "GET")
			return
		}
		s.handleDBDrivers(sw, requestID)
	case path == "/v1/db/query":
		if r.Method != "POST" {
			s.methodNotAllowed(sw, requestID, "POST")
			return
		}
		s.handleDBQuery(sw, r, requestID)
	case path == "/mcp":
		s.handleMCP(sw, r, requestID)
	default:
		s.writeError(sw, requestID, apiError("not_found", "no such endpoint", 404))
	}
}

func setPairCORS(w http.ResponseWriter) {
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("Access-Control-Allow-Methods", "POST, OPTIONS")
	w.Header().Set("Access-Control-Allow-Headers", "Content-Type")
	w.Header().Set("Access-Control-Max-Age", "600")
}

func (s *agentServer) handlePair(w http.ResponseWriter, r *http.Request, requestID string) {
	var req PairRequest
	if apiErr := s.readJSON(w, r, &req, 64<<10); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	if apiErr := s.pairing.redeem(req.Code, remoteHost(r)); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	hostname, _ := os.Hostname()
	s.writeResult(w, http.StatusOK, requestID, PairResult{
		Token:     s.cfg.Token,
		AgentID:   s.cfg.AgentID,
		Name:      hostname,
		Hostname:  hostname,
		Version:   version,
		Platform:  "windows",
		Port:      s.cfg.Port,
		Addresses: localIPv4Addresses(),
	})
}

func (s *agentServer) handlePairCode(w http.ResponseWriter, requestID string) {
	state, err := s.pairing.regenerate()
	if err != nil {
		s.writeError(w, requestID, apiError("internal_error", "failed to generate pairing code", 500))
		return
	}
	hostname, _ := os.Hostname()
	s.writeResult(w, http.StatusOK, requestID, PairCodeResult{
		Code:      state.Code,
		ExpiresAt: state.ExpiresAt.UnixNano() / int64(time.Millisecond),
		Port:      s.cfg.Port,
		Name:      hostname,
		Addresses: localIPv4Addresses(),
	})
}

func (s *agentServer) handleInfo(w http.ResponseWriter, requestID string) {
	info, apiErr := s.infoResult()
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, info)
}

func (s *agentServer) panelAccessError() *APIError {
	if !panelSupported() {
		return apiError("capability_unavailable", "panel UI is only supported on Windows", 501)
	}
	if !s.cfg.AllowDesktopControl {
		return capabilityDisabled("desktop control")
	}
	return nil
}

func (s *agentServer) handlePanel(w http.ResponseWriter, r *http.Request, requestID string) {
	if apiErr := s.panelAccessError(); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	switch r.Method {
	case "GET":
		status := s.panel.status()
		s.writeResult(w, http.StatusOK, requestID, status)
	case "POST":
		var content PanelContent
		if apiErr := s.readJSON(w, r, &content, maxPanelBody); apiErr != nil {
			s.writeError(w, requestID, apiErr)
			return
		}
		status, apiErr := s.panel.set(content)
		if apiErr != nil {
			s.writeError(w, requestID, apiErr)
			return
		}
		showAgentPanel()
		// 面板可能刚被创建为隐藏窗口；show 之后再刷一次，保证状态里的 visible 是最新值。
		updated := s.panel.status()
		if updated.Visible {
			status = &updated
		}
		s.writeResult(w, http.StatusOK, requestID, status)
	case "DELETE":
		status := s.panel.clear()
		notifyPanelChanged()
		s.writeResult(w, http.StatusOK, requestID, status)
	default:
		s.methodNotAllowed(w, requestID, "GET, POST, DELETE")
	}
}

func (s *agentServer) handlePanelActions(w http.ResponseWriter, r *http.Request, requestID string) {
	if apiErr := s.panelAccessError(); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	// ?peek=1 只读不消费；默认消费。
	consume := r.URL.Query().Get("peek") != "1"
	actions := s.panel.takeActions(consume)
	s.writeResult(w, http.StatusOK, requestID, map[string]interface{}{"actions": actions})
}

func (s *agentServer) capabilities() Capabilities {
	return Capabilities{
		Shell:          s.cfg.AllowShell,
		Write:          s.cfg.AllowWrite,
		Delete:         s.cfg.AllowDelete,
		DesktopControl: s.cfg.AllowDesktopControl,
		Database:       s.cfg.AllowDB,
		DatabaseWrite:  s.cfg.AllowDBWrite,
		Panel:          panelSupported() && s.cfg.AllowDesktopControl,
	}
}

func (s *agentServer) handleExec(w http.ResponseWriter, r *http.Request, requestID string) {
	var req ExecRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.execSync(r.Context(), req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleJobStart(w http.ResponseWriter, r *http.Request, requestID string) {
	var req ExecRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.startBackgroundJob(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleJobSubpath(w http.ResponseWriter, r *http.Request, requestID string) {
	rest := strings.TrimPrefix(r.URL.Path, "/v1/jobs/")
	parts := strings.Split(rest, "/")
	if len(parts) == 0 || parts[0] == "" {
		s.writeError(w, requestID, apiError("job_not_found", "job not found", 404))
		return
	}
	job, apiErr := s.getJob(parts[0])
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	if len(parts) == 1 {
		if r.Method != "GET" {
			s.methodNotAllowed(w, requestID, "GET")
			return
		}
		stdoutOffset := queryInt64(r, "stdoutOffset")
		stderrOffset := queryInt64(r, "stderrOffset")
		encoding := r.URL.Query().Get("encoding")
		result := job.snapshot(stdoutOffset, stderrOffset, encoding)
		s.writeResult(w, http.StatusOK, requestID, result)
		return
	}
	if len(parts) != 2 {
		s.writeError(w, requestID, apiError("not_found", "no such endpoint", 404))
		return
	}
	switch parts[1] {
	case "stdin":
		if r.Method != "POST" {
			s.methodNotAllowed(w, requestID, "POST")
			return
		}
		var req JobStdinRequest
		if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
			s.writeError(w, requestID, apiErr)
			return
		}
		var data []byte
		if strings.EqualFold(req.Encoding, "base64") {
			decoded, err := decodeBase64(req.Data)
			if err != nil {
				s.writeError(w, requestID, invalidArg("invalid base64 stdin"))
				return
			}
			data = decoded
		} else {
			data = []byte(req.Data)
		}
		n, err := job.writeStdin(data, req.Close)
		if err != nil {
			s.writeError(w, requestID, apiError("job_stdin_failed", err.Error(), 409))
			return
		}
		s.writeResult(w, http.StatusOK, requestID, JobStdinResult{JobID: job.ID, Bytes: n, Closed: req.Close})
	case "cancel":
		if r.Method != "POST" {
			s.methodNotAllowed(w, requestID, "POST")
			return
		}
		job.terminateAndWait("canceled")
		result := job.snapshot(0, 0, "")
		s.writeResult(w, http.StatusOK, requestID, result)
	default:
		s.writeError(w, requestID, apiError("not_found", "no such endpoint", 404))
	}
}

func (s *agentServer) handleFSList(w http.ResponseWriter, r *http.Request, requestID string) {
	var req FSListRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.fsList(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleFSRead(w http.ResponseWriter, r *http.Request, requestID string) {
	var req FSReadRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.fsRead(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleFSWrite(w http.ResponseWriter, r *http.Request, requestID string) {
	var req FSWriteRequest
	if apiErr := s.readJSON(w, r, &req, maxFSWriteBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.fsWrite(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleFSStat(w http.ResponseWriter, r *http.Request, requestID string) {
	var req FSListRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.fsStat(req.Path)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleFSRemove(w http.ResponseWriter, r *http.Request, requestID string) {
	var req FSRemoveRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := s.fsRemove(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleProcessList(w http.ResponseWriter, requestID string) {
	result, apiErr := processList()
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleProcessKill(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowShell {
		s.writeError(w, requestID, capabilityDisabled("shell"))
		return
	}
	var req ProcessKillRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := processKill(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleScreen(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowDesktopControl {
		s.writeError(w, requestID, capabilityDisabled("desktop control"))
		return
	}
	var req ScreenRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	select {
	case s.desktopSem <- struct{}{}:
	default:
		s.writeError(w, requestID, apiError("busy", "another desktop operation is running", 429))
		return
	}
	defer func() { <-s.desktopSem }()
	result, apiErr := captureScreen(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleMouse(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowDesktopControl {
		s.writeError(w, requestID, capabilityDisabled("desktop control"))
		return
	}
	var req MouseRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	select {
	case s.desktopSem <- struct{}{}:
	default:
		s.writeError(w, requestID, apiError("busy", "another desktop operation is running", 429))
		return
	}
	defer func() { <-s.desktopSem }()
	result, apiErr := injectMouse(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleKeyboard(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowDesktopControl {
		s.writeError(w, requestID, capabilityDisabled("desktop control"))
		return
	}
	var req KeyboardRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	select {
	case s.desktopSem <- struct{}{}:
	default:
		s.writeError(w, requestID, apiError("busy", "another desktop operation is running", 429))
		return
	}
	defer func() { <-s.desktopSem }()
	result, apiErr := injectKeyboard(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleWindowList(w http.ResponseWriter, requestID string) {
	if !s.cfg.AllowDesktopControl {
		s.writeError(w, requestID, capabilityDisabled("desktop control"))
		return
	}
	result, apiErr := listWindows()
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleWindowFocus(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowDesktopControl {
		s.writeError(w, requestID, capabilityDisabled("desktop control"))
		return
	}
	var req WindowFocusRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	result, apiErr := focusWindow(req)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleDBDrivers(w http.ResponseWriter, requestID string) {
	if !s.cfg.AllowDB {
		s.writeError(w, requestID, capabilityDisabled("database"))
		return
	}
	select {
	case s.dbSem <- struct{}{}:
	default:
		s.writeError(w, requestID, apiError("busy", "too many concurrent database operations", 429))
		return
	}
	defer func() { <-s.dbSem }()
	result, apiErr := listDBDrivers()
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) handleDBQuery(w http.ResponseWriter, r *http.Request, requestID string) {
	if !s.cfg.AllowDB {
		s.writeError(w, requestID, capabilityDisabled("database"))
		return
	}
	var req DBQueryRequest
	if apiErr := s.readJSON(w, r, &req, maxJSONBody); apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	select {
	case s.dbSem <- struct{}{}:
	default:
		s.writeError(w, requestID, apiError("busy", "too many concurrent database operations", 429))
		return
	}
	defer func() { <-s.dbSem }()
	result, apiErr := queryDB(req, s.cfg)
	if apiErr != nil {
		s.writeError(w, requestID, apiErr)
		return
	}
	s.writeResult(w, http.StatusOK, requestID, result)
}

func (s *agentServer) readJSON(w http.ResponseWriter, r *http.Request, dst interface{}, limit int64) *APIError {
	if r.Body == nil {
		return invalidArg("request body is required")
	}
	r.Body = http.MaxBytesReader(w, r.Body, limit)
	decoder := json.NewDecoder(r.Body)
	if err := decoder.Decode(dst); err != nil {
		if strings.Contains(strings.ToLower(err.Error()), "too large") {
			return apiError("payload_too_large", "request body is too large", 413)
		}
		return invalidArg("invalid JSON body: " + err.Error())
	}
	return nil
}

func (s *agentServer) writeResult(w http.ResponseWriter, status int, requestID string, result interface{}) {
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(apiEnvelope{OK: true, Result: result, RequestID: requestID})
}

func (s *agentServer) writeError(w http.ResponseWriter, requestID string, apiErr *APIError) {
	if apiErr == nil {
		apiErr = apiError("internal_error", "internal error", 500)
	}
	status := apiErr.Status
	if status == 0 {
		status = 500
	}
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.Header().Set("X-Mode-Error-Code", apiErr.Code)
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(apiEnvelope{OK: false, Error: apiErr, RequestID: requestID})
}

func (s *agentServer) methodNotAllowed(w http.ResponseWriter, requestID string, allowed string) {
	w.Header().Set("Allow", allowed)
	s.writeError(w, requestID, apiError("method_not_allowed", "method not allowed", 405))
}

func (s *agentServer) tokenMatches(presented string) bool {
	if presented == "" || s.cfg.Token == "" {
		return false
	}
	got := sha256.Sum256([]byte(presented))
	want := sha256.Sum256([]byte(s.cfg.Token))
	return subtle.ConstantTimeCompare(got[:], want[:]) == 1
}

func extractToken(r *http.Request) string {
	auth := strings.TrimSpace(r.Header.Get("Authorization"))
	if len(auth) >= 7 && strings.EqualFold(auth[:7], "bearer ") {
		return strings.TrimSpace(auth[7:])
	}
	return strings.TrimSpace(r.Header.Get("X-Mode-Agent-Token"))
}

func (s *agentServer) ipAllowed(r *http.Request) bool {
	if len(s.allowCIDRs) == 0 {
		return true
	}
	host := remoteHost(r)
	ip := net.ParseIP(host)
	if ip == nil {
		return false
	}
	for _, network := range s.allowCIDRs {
		if network.Contains(ip) {
			return true
		}
		if ip4 := ip.To4(); ip4 != nil && network.Contains(ip4) {
			return true
		}
	}
	return false
}

func remoteHost(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return r.RemoteAddr
	}
	return host
}

func parseCIDRs(values []string) ([]*net.IPNet, error) {
	result := make([]*net.IPNet, 0, len(values))
	for _, value := range values {
		value = strings.TrimSpace(value)
		if value == "" {
			continue
		}
		if !strings.Contains(value, "/") {
			ip := net.ParseIP(value)
			if ip == nil {
				return nil, fmt.Errorf("invalid allowCidrs entry: %s", value)
			}
			if ip.To4() != nil {
				value += "/32"
			} else {
				value += "/128"
			}
		}
		_, network, err := net.ParseCIDR(value)
		if err != nil {
			return nil, fmt.Errorf("invalid allowCidrs entry %s: %v", value, err)
		}
		result = append(result, network)
	}
	return result, nil
}

func queryInt64(r *http.Request, key string) int64 {
	value, err := strconv.ParseInt(r.URL.Query().Get(key), 10, 64)
	if err != nil || value < 0 {
		return 0
	}
	return value
}

func decodeBase64(value string) ([]byte, error) {
	data, err := base64.StdEncoding.DecodeString(value)
	if err == nil {
		return data, nil
	}
	return base64.RawStdEncoding.DecodeString(value)
}
