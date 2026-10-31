package pii

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/pii/guard"
	"github.com/yetone/magpie/internal/redact"
)

// gateFinder is a model whose reads wait at a gate, counting how many
// are under way at once.
type gateFinder struct {
	gate      chan struct{}
	now, most *atomic.Int32
}

func (f gateFinder) Find(text string) []guard.Span {
	n := f.now.Add(1)
	for m := f.most.Load(); n > m && !f.most.CompareAndSwap(m, n); m = f.most.Load() {
	}
	<-f.gate
	f.now.Add(-1)
	return nil
}

// yyhhyyyyyy on Discord: AstrLink Guard read one request at a time, so
// agents' requests at once queued behind each other. It reads
// guard.Cores() of them at once now, and they load the model once.
func TestGuardReadsRequestsAtOnce(t *testing.T) {
	k := cap(guardTurn)
	if k != guard.Cores() {
		t.Fatalf("%d turns, %d cores", k, guard.Cores())
	}
	if k < 2 {
		t.Skipf("one core to read on here (GOMAXPROCS %d)", guard.Cores())
	}
	guardSandbox(t)
	fakeGuardFiles(t)
	var loads, now, most atomic.Int32
	loading, gate := make(chan struct{}), make(chan struct{})
	guardLoad = func(string) (spanFinder, error) {
		loads.Add(1)
		<-loading
		return gateFinder{gate, &now, &most}, nil
	}
	var wg sync.WaitGroup
	errs := make(chan error, k)
	for i := range k {
		wg.Go(func() {
			_, err := Of(AstrLinkGuard).Find(context.Background(), []string{fmt.Sprintf("request %d", i)})
			errs <- err
		})
	}
	waitFor(t, "every request its turn", func() bool { return len(guardTurn) == k })
	// the other requests wait for the first's load: watch a while for
	// one loading beside it
	for range 40 {
		if n := loads.Load(); n != 1 {
			t.Fatalf("%d loads for requests at once", n)
		}
		time.Sleep(5 * time.Millisecond)
	}
	close(loading)
	waitFor(t, "every request read at once", func() bool { return now.Load() == int32(k) })
	close(gate)
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	if loads.Load() != 1 || most.Load() != int32(k) {
		t.Fatalf("%d loads, %d read at most at once", loads.Load(), most.Load())
	}
}

// The idle timer lets the model go only with no request on it: with one
// turn of several held, it waits for the next timer.
func TestGuardIdleWaitsForEveryTurn(t *testing.T) {
	guardSandbox(t)
	fakeGuardFiles(t)
	var reads atomic.Int32
	guardLoad = func(string) (spanFinder, error) { return fakeFinder{nil, &reads}, nil }
	if _, err := Of(AstrLinkGuard).Find(context.Background(), []string{"one"}); err != nil {
		t.Fatal(err)
	}
	guardTurn <- struct{}{} // a request on it
	guardIdle()
	running := Of(AstrLinkGuard).Now().Running
	<-guardTurn
	if !running {
		t.Fatal("let go while a request read it")
	}
	if len(guardTurn) != 0 {
		t.Fatalf("%d turns left held", len(guardTurn))
	}
	guardIdle()
	if Of(AstrLinkGuard).Now().Running {
		t.Fatal("kept with no request")
	}
}

// Requests read at once don't read one text beside each other: the
// second waits for the first's read and takes its answer (subagents
// started at once send one system prompt each).
func TestOneTextReadOnceAtOnce(t *testing.T) {
	dir := t.TempDir()
	var calls atomic.Int32
	first, release := make(chan struct{}), make(chan struct{})
	b := &backend{turn: make(chan struct{}, 2), waiting: new(atomic.Int32), cache: newLRU(100), disk: new(diskState),
		path:     func() string { return filepath.Join(dir, "found.bin") },
		heldBack: func() error { return nil },
		chunk:    guardChunk,
		classify: func(texts []string) ([][]redact.Found, error) {
			if calls.Add(1) == 1 {
				close(first)
				<-release
			}
			out := make([][]redact.Found, len(texts))
			for i, s := range texts {
				if j := strings.Index(s, "13800138000"); j >= 0 {
					out[i] = []redact.Found{{Start: j, End: j + 11, Label: "private_phone", Score: 0.9}}
				}
			}
			return out, nil
		}}
	t.Cleanup(func() {
		b.disk.mu.Lock()
		if b.disk.timer != nil && b.disk.timer.Stop() {
			b.disk.saves.Done()
		}
		b.disk.timer = nil
		b.disk.mu.Unlock()
		b.disk.saves.Wait()
	})
	texts := []string{"the system prompt, call 13800138000"}
	got := make(chan [][]redact.Found, 2)
	find := func() {
		out, err := b.find(context.Background(), texts)
		if err != nil {
			t.Error(err)
		}
		got <- out
	}
	go find()
	<-first
	go find()
	waitFor(t, "the second request its turn", func() bool { return len(b.turn) == 2 })
	// it waits for the first's read: watch a while for a read beside it
	for range 40 {
		if n := calls.Load(); n != 1 {
			t.Fatalf("%d reads of one text", n)
		}
		time.Sleep(5 * time.Millisecond)
	}
	close(release)
	for range 2 {
		out := <-got
		if len(out) != 1 || len(out[0]) != 1 || out[0][0].Label != "private_phone" {
			t.Fatalf("found %v", out)
		}
	}
	if n := calls.Load(); n != 1 {
		t.Fatalf("%d reads of one text", n)
	}
}
