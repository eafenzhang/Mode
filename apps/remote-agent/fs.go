package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"io"
	"io/ioutil"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf16"
	"unicode/utf8"
)

const (
	fsReadDefaultBytes = 1 << 20
	fsReadMaxBytes     = 16 << 20
	fsListDefault      = 1000
	fsListMax          = 10000
)

func (s *agentServer) resolvePath(raw string, forWrite bool) (string, *APIError) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return "", invalidArg("path is required")
	}
	if strings.HasPrefix(raw, `\\?\`) || strings.HasPrefix(raw, `\\.\`) {
		return "", invalidArg("Windows device paths are not allowed")
	}
	abs, err := filepath.Abs(raw)
	if err != nil {
		return "", invalidArg("invalid path: " + raw)
	}
	abs = filepath.Clean(abs)
	if len(s.cfg.AllowedRoots) == 0 {
		return abs, nil
	}
	target := resolveExistingAncestor(abs)
	for _, root := range s.cfg.AllowedRoots {
		rootAbs := resolveExistingAncestor(filepath.Clean(root))
		rel, err := filepath.Rel(rootAbs, target)
		if err != nil {
			continue
		}
		if rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(os.PathSeparator))) {
			return abs, nil
		}
	}
	return "", pathNotAllowed(abs)
}

// 先解析目标路径中已存在的最长祖先，再把不存在的后缀拼回去。只解析完整路径不足以覆盖写入：
// 8.3 短路径根与长路径目标会被 filepath.Rel 误判为不同前缀，即使目标其实在根内。
func resolveExistingAncestor(path string) string {
	current := filepath.Clean(path)
	suffix := ""
	for {
		if resolved, err := filepath.EvalSymlinks(current); err == nil {
			if suffix == "" {
				return resolved
			}
			return filepath.Join(resolved, suffix)
		}
		parent := filepath.Dir(current)
		if parent == current {
			return filepath.Clean(path)
		}
		suffix = filepath.Join(filepath.Base(current), suffix)
		current = parent
	}
}

func fsError(err error, path string) *APIError {
	if os.IsNotExist(err) {
		return apiError("not_found", "path not found: "+path, 404)
	}
	if os.IsPermission(err) {
		return apiError("forbidden", "permission denied: "+path, 403)
	}
	return apiError("fs_error", err.Error(), 500)
}

func (s *agentServer) fsList(req FSListRequest) (*FSListResult, *APIError) {
	path, apiErr := s.resolvePath(req.Path, false)
	if apiErr != nil {
		return nil, apiErr
	}
	infos, err := ioutil.ReadDir(path)
	if err != nil {
		return nil, fsError(err, path)
	}
	limit := req.MaxEntries
	if limit <= 0 {
		limit = fsListDefault
	}
	if limit > fsListMax {
		limit = fsListMax
	}
	truncated := false
	if len(infos) > limit {
		infos = infos[:limit]
		truncated = true
	}
	entries := make([]FSEntry, 0, len(infos))
	for _, info := range infos {
		entries = append(entries, FSEntry{
			Name:     info.Name(),
			Path:     filepath.Join(path, info.Name()),
			Dir:      info.IsDir(),
			Size:     info.Size(),
			Modified: nowRFC3339(info.ModTime()),
			Mode:     info.Mode().String(),
		})
	}
	return &FSListResult{Path: path, Entries: entries, Truncated: truncated}, nil
}

func (s *agentServer) fsStat(reqPath string) (*FSStatResult, *APIError) {
	path, apiErr := s.resolvePath(reqPath, false)
	if apiErr != nil {
		return nil, apiErr
	}
	info, err := os.Stat(path)
	if err != nil {
		return nil, fsError(err, path)
	}
	return &FSStatResult{
		Path:     path,
		Name:     info.Name(),
		Dir:      info.IsDir(),
		Size:     info.Size(),
		Modified: nowRFC3339(info.ModTime()),
		Mode:     info.Mode().String(),
	}, nil
}

func (s *agentServer) fsRead(req FSReadRequest) (*FSReadResult, *APIError) {
	path, apiErr := s.resolvePath(req.Path, false)
	if apiErr != nil {
		return nil, apiErr
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, fsError(err, path)
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, fsError(err, path)
	}
	if info.IsDir() {
		return nil, invalidArg("path is a directory: " + path)
	}
	if req.Offset < 0 {
		return nil, invalidArg("offset must be >= 0")
	}
	maxBytes := req.MaxBytes
	if maxBytes <= 0 {
		maxBytes = fsReadDefaultBytes
	}
	if maxBytes > fsReadMaxBytes {
		maxBytes = fsReadMaxBytes
	}
	size := info.Size()
	offset := req.Offset
	if offset > size {
		offset = size
	}
	remaining := size - offset
	readSize := int64(maxBytes)
	if readSize > remaining {
		readSize = remaining
	}
	buf := make([]byte, int(readSize))
	n, readErr := file.ReadAt(buf, offset)
	if readErr != nil && readErr != io.EOF {
		return nil, fsError(readErr, path)
	}
	buf = buf[:n]
	next := offset + int64(n)
	truncated := next < size

	result := &FSReadResult{
		Path:       path,
		Size:       size,
		Offset:     offset,
		NextOffset: next,
		Truncated:  truncated,
	}
	mode := strings.ToLower(strings.TrimSpace(req.Encoding))
	if mode == "" {
		mode = "auto"
	}
	if mode == "binary" {
		result.Encoding = "base64"
		result.Base64 = base64.StdEncoding.EncodeToString(buf)
		return result, nil
	}
	if bytes.HasPrefix(buf, []byte{0xEF, 0xBB, 0xBF}) {
		result.Encoding = "utf-8"
		result.Text = sanitizeUTF8(buf[3:])
		return result, nil
	}
	if len(buf) >= 2 && buf[0] == 0xFF && buf[1] == 0xFE {
		result.Encoding = "utf-16le"
		result.Text = decodeUTF16(buf[2:], false)
		return result, nil
	}
	if len(buf) >= 2 && buf[0] == 0xFE && buf[1] == 0xFF {
		result.Encoding = "utf-16be"
		result.Text = decodeUTF16(buf[2:], true)
		return result, nil
	}
	if mode == "oem" || mode == "ansi" {
		result.Encoding = mode
		result.Text = decodeConsoleBytes(buf, mode)
		return result, nil
	}
	if mode == "utf8" || mode == "utf-8" {
		result.Encoding = "utf-8"
		result.Text = sanitizeUTF8(buf)
		return result, nil
	}
	if bytes.IndexByte(buf, 0) >= 0 {
		result.Encoding = "base64"
		result.Base64 = base64.StdEncoding.EncodeToString(buf)
		return result, nil
	}
	if utf8.Valid(buf) {
		result.Encoding = "utf-8"
		result.Text = string(buf)
		return result, nil
	}
	result.Encoding = "oem"
	result.Text = decodeConsoleBytes(buf, "auto")
	return result, nil
}

func decodeUTF16(data []byte, bigEndian bool) string {
	if len(data)%2 != 0 {
		data = data[:len(data)-1]
	}
	units := make([]uint16, len(data)/2)
	for i := 0; i < len(units); i++ {
		if bigEndian {
			units[i] = uint16(data[2*i])<<8 | uint16(data[2*i+1])
		} else {
			units[i] = uint16(data[2*i+1])<<8 | uint16(data[2*i])
		}
	}
	return string(utf16.Decode(units))
}

func (s *agentServer) fsWrite(req FSWriteRequest) (*FSWriteResult, *APIError) {
	if !s.cfg.AllowWrite {
		return nil, capabilityDisabled("write")
	}
	path, apiErr := s.resolvePath(req.Path, true)
	if apiErr != nil {
		return nil, apiErr
	}
	var data []byte
	if req.Text != "" && req.Base64 != "" {
		return nil, invalidArg("text and base64 are mutually exclusive")
	}
	if req.Base64 != "" {
		decoded, err := base64.StdEncoding.DecodeString(req.Base64)
		if err != nil {
			decoded, err = base64.RawStdEncoding.DecodeString(req.Base64)
			if err != nil {
				return nil, invalidArg("base64 payload is invalid")
			}
		}
		data = decoded
	} else {
		data = []byte(req.Text)
	}
	dir := filepath.Dir(path)
	if req.CreateDirs {
		if err := os.MkdirAll(dir, 0755); err != nil {
			return nil, fsError(err, dir)
		}
	}
	sum := sha256.Sum256(data)
	sumHex := hex.EncodeToString(sum[:])
	if req.ExpectedSHA256 != "" && !strings.EqualFold(strings.TrimSpace(req.ExpectedSHA256), sumHex) {
		return nil, apiError("checksum_mismatch", "expected sha256 does not match payload", 400)
	}
	if req.Append {
		file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0644)
		if err != nil {
			return nil, fsError(err, path)
		}
		n, writeErr := file.Write(data)
		closeErr := file.Close()
		if writeErr != nil {
			return nil, fsError(writeErr, path)
		}
		if closeErr != nil {
			return nil, fsError(closeErr, path)
		}
		return &FSWriteResult{Path: path, Bytes: int64(n), SHA256: sumHex}, nil
	}
	tmp, err := ioutil.TempFile(dir, ".mode-agent-")
	if err != nil {
		return nil, fsError(err, dir)
	}
	tmpName := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			tmp.Close()
			os.Remove(tmpName)
		}
	}()
	n, writeErr := tmp.Write(data)
	if writeErr != nil {
		return nil, fsError(writeErr, tmpName)
	}
	if err := tmp.Sync(); err != nil {
		return nil, fsError(err, tmpName)
	}
	if err := tmp.Close(); err != nil {
		return nil, fsError(err, tmpName)
	}
	if err := replaceFile(tmpName, path); err != nil {
		return nil, fsError(err, path)
	}
	cleanup = false
	return &FSWriteResult{Path: path, Bytes: int64(n), SHA256: sumHex}, nil
}

func (s *agentServer) fsRemove(req FSRemoveRequest) (*FSRemoveResult, *APIError) {
	if !s.cfg.AllowDelete {
		return nil, capabilityDisabled("delete")
	}
	path, apiErr := s.resolvePath(req.Path, true)
	if apiErr != nil {
		return nil, apiErr
	}
	if isVolumeRoot(path) {
		return nil, apiError("path_not_allowed", "refusing to delete a volume root: "+path, 403)
	}
	if _, err := os.Lstat(path); err != nil {
		return nil, fsError(err, path)
	}
	if req.Recursive {
		if err := os.RemoveAll(path); err != nil {
			return nil, fsError(err, path)
		}
	} else if err := os.Remove(path); err != nil {
		return nil, fsError(err, path)
	}
	return &FSRemoveResult{Path: path, Removed: true}, nil
}

func isVolumeRoot(path string) bool {
	cleaned := filepath.Clean(path)
	volume := filepath.VolumeName(cleaned)
	if volume != "" {
		root := filepath.Clean(volume + string(os.PathSeparator))
		return strings.EqualFold(cleaned, root)
	}
	return cleaned == string(os.PathSeparator)
}
