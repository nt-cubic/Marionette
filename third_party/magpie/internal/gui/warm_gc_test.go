package gui

import (
	"runtime/debug"
	"testing"
)

func gcPercent() int {
	p := debug.SetGCPercent(100)
	debug.SetGCPercent(p)
	return p
}

// The usage page's read of the whole history runs under a tighter GOGC, so
// its heap (every row at once) isn't let grow to twice that, and puts back
// what was set when it ends; a lower GOGC, or off, is kept as it is.
func TestUnderTightGC(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	for _, before := range []int{100, 200, 10, -1} {
		debug.SetGCPercent(before)
		var during int
		underTightGC(func() { during = gcPercent() })
		want := min(before, tightGCPercent)
		if before < 0 {
			want = before
		}
		if during != want {
			t.Errorf("GOGC=%d: read under %d, want %d", before, during, want)
		}
		if after := gcPercent(); after != before {
			t.Errorf("GOGC=%d: after the read it is %d", before, after)
		}
	}
}

// The warm-up's usage read is the one under it, and the warm-up's other
// reads are not.
func TestWarmUpTightensOnlyTheUsageRead(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	var during int
	warmUp(func() { during = gcPercent() })
	if during != 100 {
		t.Fatalf("a warm-up step read under GOGC=%d, want 100", during)
	}
}
