package maskrec

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The body a request went with is kept in memory for the latest keepSent
// requests only, never on disk, and not past sentMax; what the answer put
// back is memory's too, added to as it is told.
func TestSentAndBackInMemory(t *testing.T) {
	now := time.Date(2026, 10, 24, 9, 0, 0, 0, time.Local)
	dir := sandbox(t, now)
	const p = "{{API_KEY_ccccdddd}}"
	body := `{"messages":[{"role":"user","content":"key ` + p + ` here"}]}`
	first := Add(Record{Time: now, What: "chat", N: 1, Took: 812, Sent: body,
		Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Head: "sk-", Len: 40}}})
	if first.Sent != "" {
		t.Fatal("Add handed the body back")
	}
	if got, ok := SentOf(first.Key); !ok || got != body {
		t.Fatalf("SentOf: %q %v", got, ok)
	}
	Restored(first.Key, map[string]int{p: 2})
	Restored(first.Key, map[string]int{p: 1})
	got, ok := Get(first.Key)
	if !ok || got.Back[p] != 3 || got.Took != 812 || got.Sent != "" {
		t.Fatalf("Get: %+v", got)
	}
	b, err := os.ReadFile(filepath.Join(dir, "2026-10-24.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(b), "here") || strings.Contains(string(b), `"back"`) || !strings.Contains(string(b), `"took":812`) {
		t.Fatalf("disk: %s", b)
	}

	// too large: the record is kept, its body isn't
	big := Add(Record{Time: now, What: "chat", N: 1, Sent: strings.Repeat("x", sentMax+1),
		Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Len: 40}}})
	if _, ok := SentOf(big.Key); ok {
		t.Fatal("a body over sentMax was kept")
	}
	// keepSent more: the first's body goes, its record and snips stay
	for range keepSent {
		Add(Record{Time: now, What: "chat", N: 1, Sent: body,
			Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Len: 40}}})
	}
	if _, ok := SentOf(first.Key); ok {
		t.Fatal("the oldest body outlived keepSent")
	}
	if got, ok := Get(first.Key); !ok || got.Back[p] != 3 {
		t.Fatalf("the record went with its body: %+v %v", got, ok)
	}
	store.mu.Lock()
	n := 0
	for _, r := range store.snips {
		if r.Sent != "" {
			n++
		}
	}
	store.mu.Unlock()
	if n != keepSent {
		t.Fatalf("%d bodies kept, want %d", n, keepSent)
	}
	// sentBudget in all: the oldest go first
	sandbox(t, now)
	var keys []int64
	for range 20 {
		r := Add(Record{Time: now, What: "chat", N: 1, Sent: strings.Repeat("y", sentMax),
			Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Len: 40}}})
		keys = append(keys, r.Key)
	}
	if _, ok := SentOf(keys[len(keys)-sentBudget/sentMax-1]); ok {
		t.Fatal("bodies over sentBudget kept")
	}
	if _, ok := SentOf(keys[len(keys)-sentBudget/sentMax]); !ok {
		t.Fatal("a body within sentBudget dropped")
	}
	if store.sentBytes != sentBudget {
		t.Fatalf("sentBytes %d", store.sentBytes)
	}
	// a key nobody has
	Restored(12345, map[string]int{p: 1})
	if _, ok := SentOf(12345); ok {
		t.Fatal("SentOf an unknown key")
	}
}
