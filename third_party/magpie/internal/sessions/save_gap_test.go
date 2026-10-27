package sessions

import (
	"sync/atomic"
	"testing"
	"time"
)

// A save right after another waits out saveGap rather than writing the
// whole index again (each Sessions poll wrote it), and Saved writes it at
// once, not after the gap.
func TestSaveWaitsOutTheGap(t *testing.T) {
	setup(t)
	var writes atomic.Int32
	old := saveGap
	saveGap = time.Hour
	saveHook = func() { writes.Add(1) }
	t.Cleanup(func() { saveGap, saveHook = old, nil })
	saved()
	saving.Lock()
	saving.wrote = time.Time{}
	saving.Unlock()

	saveCache()
	saved()
	if n := writes.Load(); n != 1 {
		t.Fatalf("%d writes of the first save, want 1", n)
	}
	saveCache()
	saveCache()
	time.Sleep(50 * time.Millisecond) // room for a write the gap should hold back
	if n := writes.Load(); n != 1 {
		t.Fatalf("%d writes with saveGap not yet past, want the first only", n)
	}
	done := make(chan struct{})
	go func() { saved(); close(done) }()
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("Saved waited out the gap")
	}
	if n := writes.Load(); n != 2 {
		t.Fatalf("%d writes once Saved, want the two saves in a row folded into one", n)
	}
}
