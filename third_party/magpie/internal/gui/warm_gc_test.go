package gui

import (
	"math"
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

// Under it no memory limit is set: the warm-up's read held at what Go
// held when it began collected every ~160 ms for 13 s of a cold start.
func TestUnderTightGCSetsNoMemoryLimit(t *testing.T) {
	defer debug.SetMemoryLimit(debug.SetMemoryLimit(math.MaxInt64))
	var during int64
	underTightGC(func() { during = debug.SetMemoryLimit(-1) })
	if during != math.MaxInt64 {
		t.Errorf("read under limit %d, want none", during)
	}
}

// Two reads under it at once put back what was set before the first, in
// either order they end: each putting back what it found left the second's
// 25 when the first ended before it.
func TestTightGCOverlappingReads(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	defer debug.SetMemoryLimit(debug.SetMemoryLimit(math.MaxInt64))
	for _, firstEndsFirst := range []bool{true, false} {
		first := tightenGC()
		second := tightenGC()
		if p := gcPercent(); p != tightGCPercent {
			t.Fatalf("two reads under GOGC=%d, want %d", p, tightGCPercent)
		}
		if firstEndsFirst {
			first()
		} else {
			second()
		}
		if p := gcPercent(); p != tightGCPercent {
			t.Errorf("one read still in flight under GOGC=%d, want %d", p, tightGCPercent)
		}
		first()
		second()
		if p := gcPercent(); p != 100 {
			t.Errorf("first ends first %v: after both GOGC=%d, want 100", firstEndsFirst, p)
		}
		if l := debug.SetMemoryLimit(-1); l != math.MaxInt64 {
			t.Errorf("first ends first %v: after both the limit is %d", firstEndsFirst, l)
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
