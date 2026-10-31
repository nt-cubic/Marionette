package maskrec

import (
	"fmt"
	"math/rand/v2"
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
	n := len(store.sent)
	for _, r := range store.snips {
		if r.Sent != "" {
			t.Fatal("a record in memory holds its body whole")
		}
	}
	store.mu.Unlock()
	if n != keepSent {
		t.Fatalf("%d bodies kept, want %d", n, keepSent)
	}
	// sentBudget in all, as stored: the oldest go first. Random bytes
	// don't compress, so each takes about sentMax.
	sandbox(t, now)
	rnd := rand.New(rand.NewPCG(1, 2))
	var keys []int64
	for range 20 {
		raw := make([]byte, sentMax)
		for i := range raw {
			raw[i] = byte(rnd.Uint32())
		}
		r := Add(Record{Time: now, What: "chat", N: 1, Sent: string(raw),
			Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Len: 40}}})
		keys = append(keys, r.Key)
	}
	store.mu.Lock()
	n, stored := len(store.sent), store.sentBytes
	store.mu.Unlock()
	if stored > sentBudget || n < sentBudget/sentMax-1 || n > sentBudget/sentMax {
		t.Fatalf("%d bodies of %d bytes kept, budget %d", n, stored, sentBudget)
	}
	if _, ok := SentOf(keys[len(keys)-n-1]); ok {
		t.Fatal("bodies over sentBudget kept")
	}
	if _, ok := SentOf(keys[len(keys)-n]); !ok {
		t.Fatal("a body within sentBudget dropped")
	}
	// a key nobody has
	Restored(12345, map[string]int{p: 1})
	if _, ok := SentOf(12345); ok {
		t.Fatal("SentOf an unknown key")
	}
}

// A request is JSON that repeats itself: memory keeps it compressed, a
// tenth of its size or less, and hands it back whole (#2278).
func TestSentKeptCompressed(t *testing.T) {
	now := time.Date(2026, 10, 24, 9, 0, 0, 0, time.Local)
	sandbox(t, now)
	const p = "{{API_KEY_ccccdddd}}"
	var b strings.Builder
	b.WriteString(`{"model":"gpt-6.1-sol","input":[`)
	for i := 0; b.Len() < 1<<20; i++ {
		fmt.Fprintf(&b, `{"type":"function_call_output","call_id":"call_%d","output":"line %d of the build log, key %s, ok\\n"},`, i, i*7, p)
	}
	b.WriteString(`{"role":"user","content":"go on"}]}`)
	body := b.String()
	r := Add(Record{Time: now, What: "chat", N: 1, Sent: body,
		Values: []Value{{Placeholder: p, Kind: "API_KEY", By: "rule", Len: 40}}})
	store.mu.Lock()
	stored := store.sentBytes
	store.mu.Unlock()
	if stored == 0 || stored > len(body)/10 {
		t.Fatalf("a %d-byte request is kept in %d bytes", len(body), stored)
	}
	if got, ok := SentOf(r.Key); !ok || got != body {
		t.Fatalf("SentOf: %d bytes, %v; want %d", len(got), ok, len(body))
	}
}
