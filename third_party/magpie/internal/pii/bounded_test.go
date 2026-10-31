package pii

import (
	"context"
	"errors"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/redact"
)

// fakeReader is a backend whose model finds "Alice" wherever it is, and
// keeps what it was asked.
type fakeReader struct {
	*backend
	mu    sync.Mutex
	asked []string
	// during, when set, runs in each read
	during func(n int)
}

func newReader(t *testing.T, chunk int) *fakeReader {
	dir := t.TempDir()
	r := &fakeReader{}
	r.backend = &backend{turn: make(chan struct{}, 1), waiting: new(atomic.Int32), cache: newLRU(10000), disk: new(diskState),
		path:     func() string { return filepath.Join(dir, "found.bin") },
		heldBack: func() error { return nil },
		chunk:    chunk,
		classify: func(texts []string) ([][]redact.Found, error) {
			r.mu.Lock()
			r.asked = append(r.asked, texts...)
			n := len(r.asked)
			r.mu.Unlock()
			if r.during != nil {
				r.during(n)
			}
			out := make([][]redact.Found, len(texts))
			for i, s := range texts {
				for at := 0; ; {
					j := strings.Index(s[at:], "Alice")
					if j < 0 {
						break
					}
					out[i] = append(out[i], redact.Found{Start: at + j, End: at + j + 5, Label: "private_person", Score: 0.9})
					at += j + 5
				}
			}
			return out, nil
		}}
	t.Cleanup(func() {
		r.disk.mu.Lock()
		if r.disk.timer != nil && r.disk.timer.Stop() {
			r.disk.saves.Done()
		}
		r.disk.timer = nil
		r.disk.mu.Unlock()
		r.disk.saves.Wait()
	})
	return r
}

// bytes is how much the model was asked to read, and how many times.
func (r *fakeReader) bytes() (n, reads int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, s := range r.asked {
		n += len(s)
	}
	return n, len(r.asked)
}

// history is n turns of about size bytes each, the last named "turn n".
func history(n, size int) []string {
	var out []string
	for i := range n {
		out = append(out, "turn "+strings.Repeat("x", 3)+string(rune('A'+i%26))+" "+strings.Repeat("words go here ", size/14))
	}
	return out
}

// An agent's history is oldest first, and what is new in a request is
// at its end: that is read first, so what a request waits for is what
// it adds, not the history behind it. Before, the oldest text was read
// first, and a history not read yet kept the newest turn waiting behind
// it on every request (xinvxueyuan and shensmart on Discord, 10-28).
func TestNewestTextReadFirst(t *testing.T) {
	r := newReader(t, 0)
	texts := []string{strings.Repeat("a ", chunkSize/2-8), strings.Repeat("b ", chunkSize/2-8), "Alice wrote: " + strings.Repeat("c ", chunkSize/2-16)}
	if _, err := r.find(context.Background(), texts); err != nil {
		t.Fatal(err)
	}
	if len(r.asked) != 3 || !strings.HasPrefix(r.asked[0], "Alice") {
		t.Fatalf("read first: %.20q of %d", r.asked[0], len(r.asked))
	}
	// a long text's last piece goes first too
	r = newReader(t, 0)
	long := strings.Repeat("a ", pieceSize) + " Alice " + strings.Repeat("z ", 100)
	if _, err := r.find(context.Background(), []string{long}); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(r.asked[0], "Alice") {
		t.Fatalf("a long text's first piece read first: %.20q", r.asked[0])
	}
}

// A Bounded request reads what it adds to a history, and of what an
// earlier request went without the model's reading, backlogCap at most.
// Before, every request read on into the whole history not read yet, on
// every core, for as long as requests came.
func TestBoundedReadsTheBacklogALittleARequest(t *testing.T) {
	r := newReader(t, guardChunk)
	turns := history(40, 3000) // 120 KB the model hasn't read
	// the first request stops waiting after one chunk, as one does at
	// its 2 s
	ctx, cancel := context.WithCancel(Bounded(context.Background()))
	r.during = func(int) { cancel() }
	got, err := r.find(ctx, turns)
	var p *Partial
	if !errors.As(err, &p) {
		t.Fatalf("a Bounded request that stopped waiting: %v", err)
	}
	if len(got) != len(turns) {
		t.Fatalf("%d answers for %d texts", len(got), len(turns))
	}
	first, _ := r.bytes()
	if first > guardChunk {
		t.Fatalf("the first request read %d bytes after it stopped waiting", first)
	}
	r.during = nil
	// the next turn: the history and a turn more, no deadline at all
	turns = append(turns, "Alice asked again")
	before, _ := r.bytes()
	got, err = r.find(Bounded(context.Background()), turns)
	if err != nil {
		t.Fatal(err)
	}
	read, _ := r.bytes()
	read -= before
	if read > len("Alice asked again")+backlogCap+guardChunk {
		t.Fatalf("the second request read %d bytes of a history the first left: the backlog wasn't capped", read)
	}
	if !strings.Contains(strings.Join(r.asked, "|"), "Alice asked again") || len(got[len(got)-1]) != 1 {
		t.Fatalf("what the request added wasn't read: %v", got[len(got)-1])
	}
	// it is read still, a little a request
	for range 3 {
		before, _ := r.bytes()
		if _, err := r.find(Bounded(context.Background()), turns); err != nil {
			t.Fatal(err)
		}
		n, _ := r.bytes()
		if n-before == 0 || n-before > backlogCap+guardChunk {
			t.Fatalf("a request read %d bytes of the backlog", n-before)
		}
	}
	// turned on by the user, a request reads all of it
	if _, err := r.find(context.Background(), turns); err != nil {
		t.Fatal(err)
	}
	if _, miss := r.known(turns, nil); len(miss) != 0 {
		t.Fatalf("%d texts unread after a request that waits for all", len(miss))
	}
}

// AstrLink Guard reads in short chunks: a request that stops waiting
// leaves the model finishing the chunk it is on, on the CPU about a
// second of 4 KB where 12 KB took several.
func TestGuardReadsShortChunks(t *testing.T) {
	if guardBackend.size() > 4<<10 {
		t.Fatalf("AstrLink Guard's chunk is %d bytes", guardBackend.size())
	}
	if pf.size() != chunkSize {
		t.Fatalf("Privacy Filter's chunk is %d bytes", pf.size())
	}
}
