package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const auditMaxBytes int64 = 10 * 1024 * 1024

type auditEntry struct {
	Time       string `json:"time"`
	RequestID  string `json:"requestId,omitempty"`
	Remote     string `json:"remote,omitempty"`
	Method     string `json:"method"`
	Path       string `json:"path"`
	Status     int    `json:"status"`
	DurationMS int64  `json:"durationMs"`
	ErrorCode  string `json:"errorCode,omitempty"`
}

type auditLogger struct {
	mu       sync.Mutex
	file     *os.File
	path     string
	size     int64
	maxBytes int64
}

func newAuditLogger(dir string) (*auditLogger, error) {
	if dir == "" {
		return nil, nil
	}
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, "audit.log")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return nil, err
	}
	info, err := file.Stat()
	var size int64
	if err == nil {
		size = info.Size()
	}
	return &auditLogger{file: file, path: path, size: size, maxBytes: auditMaxBytes}, nil
}

func (a *auditLogger) write(entry auditEntry) {
	if a == nil {
		return
	}
	if entry.Time == "" {
		entry.Time = time.Now().UTC().Format(time.RFC3339Nano)
	}
	line, err := json.Marshal(entry)
	if err != nil {
		return
	}
	line = append(line, '\n')
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.file == nil {
		return
	}
	if a.maxBytes > 0 && a.size+int64(len(line)) > a.maxBytes {
		a.rotateLocked()
	}
	if a.file == nil {
		return
	}
	n, err := a.file.Write(line)
	if err == nil {
		a.size += int64(n)
	}
}

func (a *auditLogger) rotateLocked() {
	if a.file != nil {
		a.file.Close()
		a.file = nil
	}
	rotated := a.path + ".1"
	os.Remove(rotated)
	os.Rename(a.path, rotated)
	file, err := os.OpenFile(a.path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		return
	}
	a.file = file
	a.size = 0
}

func (a *auditLogger) close() {
	if a == nil {
		return
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.file != nil {
		a.file.Close()
		a.file = nil
	}
}
