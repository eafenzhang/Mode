package main

import "testing"

func TestRingBufferKeepsNewestBytes(t *testing.T) {
	ring := newRingBuffer(4096)
	payload := make([]byte, 5000)
	for index := range payload {
		payload[index] = byte(index % 251)
	}
	if _, err := ring.Write(payload); err != nil {
		t.Fatalf("write: %v", err)
	}
	chunk, next, truncated := ring.Snapshot(0)
	if !truncated {
		t.Fatal("expected truncated snapshot")
	}
	if len(chunk) != 4096 {
		t.Fatalf("expected 4096 bytes, got %d", len(chunk))
	}
	if next != 5000 {
		t.Fatalf("expected next offset 5000, got %d", next)
	}
	if chunk[0] != payload[904] {
		t.Fatalf("expected first retained byte %d, got %d", payload[904], chunk[0])
	}
	tail, tailNext, _ := ring.Snapshot(4999)
	if len(tail) != 1 || tailNext != 5000 {
		t.Fatalf("expected one tail byte and next 5000, got len=%d next=%d", len(tail), tailNext)
	}
}

func TestRingBufferOffsetBeyondEnd(t *testing.T) {
	ring := newRingBuffer(4096)
	ring.Write([]byte("abc"))
	chunk, next, truncated := ring.Snapshot(100)
	if len(chunk) != 0 || next != 3 || truncated {
		t.Fatalf("expected empty tail, got len=%d next=%d truncated=%v", len(chunk), next, truncated)
	}
}
