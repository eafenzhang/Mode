package main

import (
	"io/ioutil"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestResolvePathSandbox(t *testing.T) {
	root, err := ioutil.TempDir("", "mode-agent-root-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	defer os.RemoveAll(root)
	server := &agentServer{cfg: &Config{AllowedRoots: []string{root}, AllowWrite: true, AllowDelete: true}}

	inside := filepath.Join(root, "inside.txt")
	if _, apiErr := server.resolvePath(inside, true); apiErr != nil {
		t.Fatalf("inside path rejected: %v", apiErr)
	}
	outside := filepath.Join(root, "..", "outside.txt")
	if _, apiErr := server.resolvePath(outside, true); apiErr == nil || apiErr.Code != "path_not_allowed" {
		t.Fatalf("expected path_not_allowed, got %+v", apiErr)
	}
	if _, apiErr := server.resolvePath(`\\?\C:\Windows\win.ini`, false); apiErr == nil {
		t.Fatal("expected device path rejection")
	}
}

func TestFileRoundTripAndRemove(t *testing.T) {
	root, err := ioutil.TempDir("", "mode-agent-fs-")
	if err != nil {
		t.Fatalf("temp dir: %v", err)
	}
	defer os.RemoveAll(root)
	server := &agentServer{cfg: &Config{AllowedRoots: []string{root}, AllowWrite: true, AllowDelete: true}}
	path := filepath.Join(root, "nested", "hello.txt")

	written, apiErr := server.fsWrite(FSWriteRequest{
		Path:       path,
		Text:       "hello 中文",
		CreateDirs: true,
	})
	if apiErr != nil {
		t.Fatalf("write: %v", apiErr)
	}
	if written.Bytes != 12 {
		t.Fatalf("expected 12 bytes, got %d", written.Bytes)
	}
	read, apiErr := server.fsRead(FSReadRequest{Path: path})
	if apiErr != nil {
		t.Fatalf("read: %v", apiErr)
	}
	if !strings.Contains(read.Text, "hello 中文") {
		t.Fatalf("unexpected read text: %q", read.Text)
	}
	listed, apiErr := server.fsList(FSListRequest{Path: filepath.Dir(path)})
	if apiErr != nil {
		t.Fatalf("list: %v", apiErr)
	}
	if len(listed.Entries) != 1 || listed.Entries[0].Name != "hello.txt" {
		t.Fatalf("unexpected entries: %+v", listed.Entries)
	}
	if _, apiErr := server.fsRemove(FSRemoveRequest{Path: path}); apiErr != nil {
		t.Fatalf("remove: %v", apiErr)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("expected file removed, stat err=%v", err)
	}
}

func TestIsVolumeRoot(t *testing.T) {
	if !isVolumeRoot(filepath.VolumeName(os.TempDir()) + string(os.PathSeparator)) {
		t.Fatal("expected volume root")
	}
	if isVolumeRoot(filepath.Join(os.TempDir(), "mode-agent")) {
		t.Fatal("did not expect a normal directory to be a volume root")
	}
}
