// +build windows

package main

import "testing"

func TestParseKeyCombo(t *testing.T) {
	modifiers, key, apiErr := parseKeyCombo("CTRL+S")
	if apiErr != nil {
		t.Fatalf("parse: %v", apiErr)
	}
	if len(modifiers) != 1 || modifiers[0] != 0x11 || key != 0x53 {
		t.Fatalf("unexpected combo: modifiers=%v key=%x", modifiers, key)
	}
	modifiers, key, apiErr = parseKeyCombo("ALT+F4")
	if apiErr != nil || len(modifiers) != 1 || modifiers[0] != 0x12 || key != 0x73 {
		t.Fatalf("unexpected ALT+F4: modifiers=%v key=%x err=%v", modifiers, key, apiErr)
	}
	if _, _, apiErr = parseKeyCombo("NOPE+X"); apiErr == nil {
		t.Fatal("expected unknown modifier error")
	}
}

func TestParseHWND(t *testing.T) {
	value, err := parseHWND("0x1f")
	if err != nil || value != 31 {
		t.Fatalf("expected 31, got %d (%v)", value, err)
	}
	if _, err := parseHWND(""); err == nil {
		t.Fatal("expected empty hwnd error")
	}
}

func TestInputRecordSize(t *testing.T) {
	if len(mouseRecord(1, 2, 3, 4)) != inputRecordSize() {
		t.Fatal("mouse record size mismatch")
	}
	if len(keyRecord(0x41, 0, 0)) != inputRecordSize() {
		t.Fatal("key record size mismatch")
	}
}

func TestWindowsVersionName(t *testing.T) {
	cases := map[[3]uint32]string{
		{5, 1, 2600}:   "Windows XP",
		{6, 1, 7601}:   "Windows 7",
		{10, 0, 19044}: "Windows 10",
		{10, 0, 22631}: "Windows 11",
	}
	for input, expected := range cases {
		if got := windowsVersionName(input[0], input[1], input[2]); got != expected {
			t.Fatalf("version %v: expected %s, got %s", input, expected, got)
		}
	}
}
