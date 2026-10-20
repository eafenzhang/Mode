package main

import "sync"

// ringBuffer keeps the newest bytes and exposes absolute offsets so a polling
// client can read incrementally without receiving the whole buffer every time.
// It intentionally uses a contiguous slice: the cap is bounded, writes are
// small, and this avoids the wrap-around edge cases of a circular index.
type ringBuffer struct {
	mu        sync.Mutex
	data      []byte
	start     int64
	total     int64
	capacity  int
	truncated bool
}

func newRingBuffer(capacity int) *ringBuffer {
	if capacity < 4096 {
		capacity = 4096
	}
	return &ringBuffer{capacity: capacity, data: make([]byte, 0, capacity)}
}

func (r *ringBuffer) Write(p []byte) (int, error) {
	if len(p) == 0 {
		return 0, nil
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.data = append(r.data, p...)
	r.total += int64(len(p))
	if len(r.data) > r.capacity {
		drop := len(r.data) - r.capacity
		copy(r.data, r.data[drop:])
		r.data = r.data[:r.capacity]
		r.start += int64(drop)
		r.truncated = true
	}
	return len(p), nil
}

func (r *ringBuffer) Snapshot(offset int64) ([]byte, int64, bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	truncated := r.truncated
	if offset < r.start {
		offset = r.start
		truncated = true
	}
	if offset > r.total {
		offset = r.total
	}
	index := offset - r.start
	if index < 0 {
		index = 0
	}
	if index > int64(len(r.data)) {
		index = int64(len(r.data))
	}
	chunk := make([]byte, int64(len(r.data))-index)
	copy(chunk, r.data[index:])
	return chunk, r.total, truncated
}

func (r *ringBuffer) Len() int64 {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.total
}
