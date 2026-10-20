package main

import (
	"strings"
	"testing"
)

func TestNegotiateMCPVersion(t *testing.T) {
	if got := negotiateMCPVersion("2025-06-18"); got != "2025-06-18" {
		t.Fatalf("expected requested version, got %s", got)
	}
	if got := negotiateMCPVersion("unknown"); got != mcpLatestVersion {
		t.Fatalf("expected latest version, got %s", got)
	}
}

func TestSanitizeUTF8(t *testing.T) {
	got := sanitizeUTF8([]byte{0xff, 'a', 0xfe})
	if !strings.Contains(got, "a") {
		t.Fatalf("expected valid rune preserved, got %q", got)
	}
	if !strings.Contains(got, string([]rune{0xFFFD})) {
		t.Fatalf("expected replacement rune, got %q", got)
	}
}

func TestExecRequestFromArgs(t *testing.T) {
	req, apiErr := execRequestFromArgs(map[string]interface{}{
		"command":   "echo hi",
		"shell":     "cmd",
		"args":      []interface{}{"a", "b"},
		"env":       map[string]interface{}{"A": "B"},
		"timeoutMs": float64(1000),
	})
	if apiErr != nil {
		t.Fatalf("unexpected error: %v", apiErr)
	}
	if req.Command != "echo hi" || req.Shell != "cmd" || len(req.Args) != 2 || req.Env["A"] != "B" || req.TimeoutMS != 1000 {
		t.Fatalf("unexpected request: %+v", req)
	}
	if _, apiErr := execRequestFromArgs(map[string]interface{}{}); apiErr == nil {
		t.Fatal("expected missing command error")
	}
}

func TestParseFingerprint(t *testing.T) {
	value := "SHA256:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
	decoded, err := parseFingerprint(value)
	if err != nil {
		t.Fatalf("parse fingerprint: %v", err)
	}
	if len(decoded) != 32 || decoded[0] != 0 || decoded[31] != 0x1f {
		t.Fatalf("unexpected fingerprint bytes: %x", decoded)
	}
	if _, err := parseFingerprint("bad"); err == nil {
		t.Fatal("expected invalid fingerprint error")
	}
}

func TestParseUint32String(t *testing.T) {
	value, err := parseUint32String("0x1f")
	if err != nil || value != 31 {
		t.Fatalf("expected 31, got %d (%v)", value, err)
	}
	if _, err := parseUint32String("nope"); err == nil {
		t.Fatal("expected parse error")
	}
}
