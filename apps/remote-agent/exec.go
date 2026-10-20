package main

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

const (
	execDefaultTimeoutMS = 60 * 1000
	execMaxTimeoutMS     = 10 * 60 * 1000
	jobMaxTimeoutMS      = 24 * 60 * 60 * 1000
	outputDefaultBytes   = 1 << 20
	outputMaxBytes       = 4 << 20
	jobRetention         = 10 * time.Minute
	maxJobs              = 64
)

type Job struct {
	ID        string
	Encoding  string
	CreatedAt time.Time
	StartedAt time.Time
	EndedAt   time.Time
	Stdout    *ringBuffer
	Stderr    *ringBuffer
	cmd       *exec.Cmd
	stdin     io.WriteCloser
	jobObject uintptr
	done      chan struct{}
	release   func()

	mu          sync.Mutex
	status      string
	exitCode    int
	termination string
	timedOut    bool
	canceled    bool
}

func newJobID() string {
	raw := make([]byte, 8)
	if _, err := rand.Read(raw); err != nil {
		return fmt.Sprintf("%x", time.Now().UnixNano())
	}
	return hex.EncodeToString(raw)
}

func validJobID(id string) bool {
	if len(id) < 8 || len(id) > 32 {
		return false
	}
	for _, r := range id {
		if (r < '0' || r > '9') && (r < 'a' || r > 'f') {
			return false
		}
	}
	return true
}

func normalizeOutputBytes(requested int) int {
	if requested <= 0 {
		return outputDefaultBytes
	}
	if requested < 4096 {
		return 4096
	}
	if requested > outputMaxBytes {
		return outputMaxBytes
	}
	return requested
}

func (s *agentServer) startJob(req ExecRequest) (*Job, *APIError) {
	if !s.cfg.AllowShell {
		return nil, capabilityDisabled("shell")
	}
	command := strings.TrimSpace(req.Command)
	if command == "" {
		return nil, invalidArg("command is required")
	}
	shell := strings.ToLower(strings.TrimSpace(req.Shell))
	if shell == "" {
		shell = "cmd"
	}
	if shell != "cmd" && shell != "powershell" && shell != "none" {
		return nil, invalidArg("shell must be cmd, powershell or none")
	}
	encoding := strings.ToLower(strings.TrimSpace(req.Encoding))
	if encoding == "" {
		encoding = "auto"
	}
	if encoding != "auto" && encoding != "utf8" && encoding != "utf-8" && encoding != "oem" && encoding != "ansi" {
		return nil, invalidArg("encoding must be auto, utf8, oem or ansi")
	}
	cwd := strings.TrimSpace(req.Cwd)
	if cwd != "" {
		resolved, apiErr := s.resolvePath(cwd, false)
		if apiErr != nil {
			return nil, apiErr
		}
		info, err := os.Stat(resolved)
		if err != nil {
			return nil, fsError(err, resolved)
		}
		if !info.IsDir() {
			return nil, invalidArg("cwd is not a directory: " + resolved)
		}
		cwd = resolved
	} else {
		cwd = defaultCommandDir()
	}
	keys := make([]string, 0, len(req.Env))
	for key := range req.Env {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	for _, key := range keys {
		if !validEnvKey(key) {
			return nil, invalidArg("invalid environment key: " + key)
		}
		if strings.IndexByte(req.Env[key], 0) >= 0 {
			return nil, invalidArg("environment value contains NUL: " + key)
		}
	}
	// 合并环境变量时去重，并保留 Windows 的 =C:=... 伪变量。
	envMap := map[string]string{}
	var pseudoEnv []string
	for _, entry := range os.Environ() {
		if strings.HasPrefix(entry, "=") {
			pseudoEnv = append(pseudoEnv, entry)
			continue
		}
		index := strings.IndexByte(entry, '=')
		if index <= 0 {
			continue
		}
		envMap[entry[:index]] = entry[index+1:]
	}
	for _, key := range keys {
		envMap[key] = req.Env[key]
	}
	envKeys := make([]string, 0, len(envMap))
	for key := range envMap {
		envKeys = append(envKeys, key)
	}
	sort.Strings(envKeys)
	env := make([]string, 0, len(envKeys)+len(pseudoEnv))
	for _, key := range envKeys {
		env = append(env, key+"="+envMap[key])
	}
	env = append(env, pseudoEnv...)

	var cmd *exec.Cmd
	switch shell {
	case "cmd":
		cmd = exec.Command("cmd.exe", "/D", "/S", "/C", command)
	case "powershell":
		if _, err := exec.LookPath("powershell.exe"); err != nil {
			return nil, apiError("exec_failed", "powershell.exe is not available on this host", 400)
		}
		cmd = exec.Command("powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
			"$OutputEncoding=[Console]::OutputEncoding=[Text.Encoding]::UTF8;"+command)
	case "none":
		if _, err := exec.LookPath(command); err != nil {
			return nil, apiError("exec_failed", "command not found: "+command, 400)
		}
		cmd = exec.Command(command, req.Args...)
	}
	cmd.Dir = cwd
	cmd.Env = env
	cmd.SysProcAttr = newSysProcAttr()

	select {
	case s.execSem <- struct{}{}:
	default:
		return nil, apiError("busy", "too many concurrent commands", 429)
	}
	release := func() { <-s.execSem }
	released := false
	releaseOnce := func() {
		if !released {
			released = true
			release()
		}
	}

	job := &Job{
		ID:        newJobID(),
		Encoding:  encoding,
		CreatedAt: time.Now(),
		Stdout:    newRingBuffer(normalizeOutputBytes(req.MaxOutputBytes)),
		Stderr:    newRingBuffer(normalizeOutputBytes(req.MaxOutputBytes)),
		done:      make(chan struct{}),
		release:   releaseOnce,
		status:    "running",
	}
	cmd.Stdout = job.Stdout
	cmd.Stderr = job.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		releaseOnce()
		return nil, apiError("exec_failed", err.Error(), 400)
	}
	job.stdin = stdin
	if err := cmd.Start(); err != nil {
		stdin.Close()
		releaseOnce()
		return nil, apiError("exec_failed", err.Error(), 400)
	}
	job.cmd = cmd
	job.StartedAt = time.Now()
	if handle := newJobObject(); handle != 0 {
		if assignJobObject(handle, cmd.Process.Pid) {
			job.jobObject = handle
		} else {
			closeJobObject(handle)
		}
	}

	go job.wait()
	s.jobsMu.Lock()
	s.jobs[job.ID] = job
	s.jobsMu.Unlock()
	return job, nil
}

func validEnvKey(key string) bool {
	if key == "" {
		return false
	}
	for _, r := range key {
		if r == '=' || r == 0 {
			return false
		}
	}
	return true
}

func (j *Job) wait() {
	err := j.cmd.Wait()
	code := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			code = exitCode(exitErr.ProcessState)
		} else {
			code = -1
		}
	}
	j.finish(code)
}

func (j *Job) finish(code int) {
	j.mu.Lock()
	if j.status != "running" {
		// terminate() already marked the final reason; keep it.
	}
	if j.termination != "" {
		j.status = j.termination
	} else if code == 0 {
		j.status = "exited"
	} else {
		j.status = "failed"
	}
	j.exitCode = code
	j.EndedAt = time.Now()
	if j.cmd != nil && j.cmd.ProcessState != nil {
		if j.StartedAt.IsZero() {
			j.StartedAt = j.CreatedAt
		}
	}
	stdin := j.stdin
	j.stdin = nil
	jobObject := j.jobObject
	j.jobObject = 0
	j.mu.Unlock()

	if stdin != nil {
		stdin.Close()
	}
	if jobObject != 0 {
		closeJobObject(jobObject)
	}
	close(j.done)
	if j.release != nil {
		j.release()
	}
}

func (j *Job) terminate(reason string) {
	j.mu.Lock()
	if j.status != "running" {
		j.mu.Unlock()
		return
	}
	if j.termination == "" {
		j.termination = reason
		j.timedOut = reason == "timed_out"
		j.canceled = reason == "canceled"
	}
	jobObject := j.jobObject
	cmd := j.cmd
	j.mu.Unlock()
	if jobObject != 0 {
		terminateJobObject(jobObject)
	}
	if cmd != nil && cmd.Process != nil {
		cmd.Process.Kill()
	}
}

func (j *Job) terminateAndWait(reason string) {
	j.terminate(reason)
	select {
	case <-j.done:
	case <-time.After(5 * time.Second):
	}
}

func (j *Job) writeStdin(data []byte, closeAfter bool) (int, error) {
	j.mu.Lock()
	defer j.mu.Unlock()
	if j.stdin == nil {
		return 0, fmt.Errorf("stdin is closed")
	}
	n, err := j.stdin.Write(data)
	if closeAfter {
		j.stdin.Close()
		j.stdin = nil
	}
	return n, err
}

func (j *Job) snapshot(stdoutOffset, stderrOffset int64, encoding string) JobStatusResult {
	if encoding == "" {
		encoding = j.Encoding
	}
	j.mu.Lock()
	status := j.status
	exitCode := j.exitCode
	timedOut := j.timedOut
	canceled := j.canceled
	createdAt := j.CreatedAt
	startedAt := j.StartedAt
	endedAt := j.EndedAt
	j.mu.Unlock()

	stdout, stdoutNext, stdoutTruncated := j.Stdout.Snapshot(stdoutOffset)
	stderr, stderrNext, stderrTruncated := j.Stderr.Snapshot(stderrOffset)
	result := JobStatusResult{
		JobID:            j.ID,
		Status:           status,
		ExitCode:         exitCode,
		TimedOut:         timedOut,
		Canceled:         canceled,
		CreatedAt:        nowRFC3339(createdAt),
		StartedAt:        nowRFC3339(startedAt),
		EndedAt:          nowRFC3339(endedAt),
		Stdout:           decodeConsoleBytes(stdout, encoding),
		StdoutNextOffset: stdoutNext,
		Stderr:           decodeConsoleBytes(stderr, encoding),
		StderrNextOffset: stderrNext,
		Truncated:        stdoutTruncated || stderrTruncated,
	}
	if !startedAt.IsZero() {
		end := endedAt
		if end.IsZero() {
			end = time.Now()
		}
		result.DurationMS = end.Sub(startedAt).Nanoseconds() / int64(time.Millisecond)
	}
	return result
}

func (j *Job) summary() JobSummary {
	j.mu.Lock()
	defer j.mu.Unlock()
	summary := JobSummary{
		JobID:     j.ID,
		Status:    j.status,
		ExitCode:  j.exitCode,
		CreatedAt: nowRFC3339(j.CreatedAt),
		StartedAt: nowRFC3339(j.StartedAt),
		EndedAt:   nowRFC3339(j.EndedAt),
	}
	if !j.StartedAt.IsZero() {
		end := j.EndedAt
		if end.IsZero() {
			end = time.Now()
		}
		summary.DurationMS = end.Sub(j.StartedAt).Nanoseconds() / int64(time.Millisecond)
	}
	return summary
}

func (s *agentServer) execSync(ctx context.Context, req ExecRequest) (*ExecResult, *APIError) {
	timeoutMS := req.TimeoutMS
	if timeoutMS <= 0 {
		timeoutMS = execDefaultTimeoutMS
	}
	if timeoutMS > execMaxTimeoutMS {
		timeoutMS = execMaxTimeoutMS
	}
	job, apiErr := s.startJob(req)
	if apiErr != nil {
		return nil, apiErr
	}
	if req.Stdin != "" {
		if _, err := job.writeStdin([]byte(req.Stdin), true); err != nil {
			job.Stderr.Write([]byte("stdin: " + err.Error() + "\n"))
		}
	} else {
		job.writeStdin(nil, true)
	}
	timer := time.NewTimer(time.Duration(timeoutMS) * time.Millisecond)
	defer timer.Stop()
	select {
	case <-job.done:
	case <-ctx.Done():
		job.terminate("canceled")
		<-job.done
	case <-timer.C:
		job.terminate("timed_out")
		<-job.done
	}
	status := job.snapshot(0, 0, "")
	return &ExecResult{
		JobID:            status.JobID,
		Status:           status.Status,
		ExitCode:         status.ExitCode,
		TimedOut:         status.TimedOut,
		Canceled:         status.Canceled,
		Stdout:           status.Stdout,
		Stderr:           status.Stderr,
		StdoutNextOffset: status.StdoutNextOffset,
		StderrNextOffset: status.StderrNextOffset,
		Truncated:        status.Truncated,
		DurationMS:       status.DurationMS,
	}, nil
}

func (s *agentServer) getJob(id string) (*Job, *APIError) {
	if !validJobID(id) {
		return nil, apiError("job_not_found", "job not found: "+id, 404)
	}
	s.jobsMu.Lock()
	job := s.jobs[id]
	s.jobsMu.Unlock()
	if job == nil {
		return nil, apiError("job_not_found", "job not found: "+id, 404)
	}
	return job, nil
}

func (s *agentServer) jobList() *JobListResult {
	s.jobsMu.Lock()
	jobs := make([]*Job, 0, len(s.jobs))
	for _, job := range s.jobs {
		jobs = append(jobs, job)
	}
	s.jobsMu.Unlock()
	sort.Slice(jobs, func(i, j int) bool {
		return jobs[i].CreatedAt.After(jobs[j].CreatedAt)
	})
	result := &JobListResult{Jobs: make([]JobSummary, 0, len(jobs))}
	for _, job := range jobs {
		result.Jobs = append(result.Jobs, job.summary())
	}
	return result
}

func (s *agentServer) gcJobs() {
	now := time.Now()
	s.jobsMu.Lock()
	for id, job := range s.jobs {
		job.mu.Lock()
		ended := job.EndedAt
		job.mu.Unlock()
		if !ended.IsZero() && now.Sub(ended) > jobRetention {
			delete(s.jobs, id)
		}
	}
	s.jobsMu.Unlock()
}

func (s *agentServer) jobCount() int {
	s.jobsMu.Lock()
	defer s.jobsMu.Unlock()
	return len(s.jobs)
}

func (s *agentServer) startBackgroundJob(req ExecRequest) (*JobStatusResult, *APIError) {
	if s.jobCount() >= maxJobs {
		s.gcJobs()
		if s.jobCount() >= maxJobs {
			return nil, apiError("busy", "too many retained jobs", 429)
		}
	}
	job, apiErr := s.startJob(req)
	if apiErr != nil {
		return nil, apiErr
	}
	if req.TimeoutMS > 0 {
		timeout := req.TimeoutMS
		if timeout > jobMaxTimeoutMS {
			timeout = jobMaxTimeoutMS
		}
		go func() {
			timer := time.NewTimer(time.Duration(timeout) * time.Millisecond)
			defer timer.Stop()
			select {
			case <-job.done:
			case <-timer.C:
				job.terminate("timed_out")
			}
		}()
	}
	if req.Stdin != "" {
		job.writeStdin([]byte(req.Stdin), false)
	}
	status := job.snapshot(0, 0, "")
	return &status, nil
}

func defaultCommandDir() string {
	if profile := os.Getenv("USERPROFILE"); profile != "" {
		return filepath.Clean(profile)
	}
	return ""
}
