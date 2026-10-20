package main

import (
	"crypto/rand"
	"crypto/subtle"
	"strings"
	"sync"
	"time"
)

const (
	pairCodeTTL       = 10 * time.Minute
	pairCodeAlphabet  = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
	pairAttemptLimit  = 5
	pairAttemptWindow = time.Minute
)

type pairCodeState struct {
	Code       string
	Normalized string
	ExpiresAt  time.Time
}

type pairAttemptState struct {
	Count   int
	ResetAt time.Time
}

type pairingState struct {
	mu       sync.Mutex
	current  *pairCodeState
	attempts map[string]*pairAttemptState
}

func newPairingState() *pairingState {
	return &pairingState{attempts: map[string]*pairAttemptState{}}
}

func generatePairCode() (*pairCodeState, error) {
	raw := make([]byte, 8)
	if _, err := rand.Read(raw); err != nil {
		return nil, err
	}
	chars := make([]byte, len(raw))
	for index, value := range raw {
		chars[index] = pairCodeAlphabet[int(value)%len(pairCodeAlphabet)]
	}
	normalized := string(chars)
	return &pairCodeState{
		Code:       normalized[:4] + "-" + normalized[4:],
		Normalized: normalized,
		ExpiresAt:  time.Now().Add(pairCodeTTL),
	}, nil
}

func normalizePairCode(value string) string {
	var builder strings.Builder
	for _, char := range strings.ToUpper(value) {
		if (char >= '0' && char <= '9') || (char >= 'A' && char <= 'Z') {
			builder.WriteRune(char)
		}
	}
	return builder.String()
}

func (p *pairingState) regenerate() (*pairCodeState, error) {
	state, err := generatePairCode()
	if err != nil {
		return nil, err
	}
	p.mu.Lock()
	p.current = state
	p.mu.Unlock()
	return state, nil
}

func (p *pairingState) ensure() (*pairCodeState, error) {
	p.mu.Lock()
	current := p.current
	p.mu.Unlock()
	if current != nil && time.Now().Before(current.ExpiresAt) {
		return current, nil
	}
	return p.regenerate()
}

func (p *pairingState) redeem(code, remote string) *APIError {
	normalized := normalizePairCode(code)
	if len(normalized) != 8 {
		return apiError("invalid_pair_code", "pairing code is invalid or expired", 401)
	}
	now := time.Now()
	p.mu.Lock()
	defer p.mu.Unlock()
	attempt := p.attempts[remote]
	if attempt == nil || now.After(attempt.ResetAt) {
		attempt = &pairAttemptState{ResetAt: now.Add(pairAttemptWindow)}
		p.attempts[remote] = attempt
	}
	if attempt.Count >= pairAttemptLimit {
		return apiError("too_many_attempts", "too many pairing attempts; wait a minute and try again", 429)
	}
	current := p.current
	if current == nil || now.After(current.ExpiresAt) {
		p.current = nil
		attempt.Count++
		return apiError("invalid_pair_code", "pairing code is invalid or expired", 401)
	}
	if subtle.ConstantTimeCompare([]byte(normalized), []byte(current.Normalized)) != 1 {
		attempt.Count++
		return apiError("invalid_pair_code", "pairing code is invalid or expired", 401)
	}
	p.current = nil
	delete(p.attempts, remote)
	return nil
}
