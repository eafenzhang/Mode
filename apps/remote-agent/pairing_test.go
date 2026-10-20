package main

import (
	"strings"
	"testing"
	"time"
)

func TestPairCodeFormatAndRedeem(t *testing.T) {
	state := newPairingState()
	code, err := state.regenerate()
	if err != nil {
		t.Fatalf("generate: %v", err)
	}
	if len(code.Code) != 9 || code.Code[4] != '-' || len(code.Normalized) != 8 {
		t.Fatalf("unexpected code shape: %+v", code)
	}
	if apiErr := state.redeem(strings.ToLower(code.Code), "127.0.0.1"); apiErr != nil {
		t.Fatalf("redeem normalized code: %v", apiErr)
	}
	if apiErr := state.redeem(code.Code, "127.0.0.1"); apiErr == nil || apiErr.Code != "invalid_pair_code" {
		t.Fatalf("expected consumed code rejection, got %+v", apiErr)
	}
}

func TestPairCodeExpiryAndRateLimit(t *testing.T) {
	state := newPairingState()
	code, err := state.regenerate()
	if err != nil {
		t.Fatalf("generate: %v", err)
	}
	state.current.ExpiresAt = time.Now().Add(-time.Second)
	if apiErr := state.redeem(code.Code, "10.0.0.1"); apiErr == nil || apiErr.Code != "invalid_pair_code" {
		t.Fatalf("expected expired code rejection, got %+v", apiErr)
	}
	for index := 0; index < pairAttemptLimit; index++ {
		if apiErr := state.redeem("AAAAAAAA", "10.0.0.2"); apiErr == nil {
			t.Fatal("expected invalid code rejection")
		}
	}
	if apiErr := state.redeem("AAAAAAAA", "10.0.0.2"); apiErr == nil || apiErr.Code != "too_many_attempts" {
		t.Fatalf("expected rate limit, got %+v", apiErr)
	}
}
