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

// Under it the memory Go holds is limited to what it held when the read
// began, and the limit set before is put back; a lower limit the user set
// is kept, and GOGC off gets none.
func TestUnderTightGCLimitsMemory(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	defer debug.SetMemoryLimit(debug.SetMemoryLimit(math.MaxInt64))
	for _, c := range []struct {
		name   string
		gogc   int
		before int64
		low    bool
	}{
		{"default", 100, math.MaxInt64, true},
		{"user limit lower", 100, 1 << 20, false},
		{"GOGC off", -1, math.MaxInt64, false},
	} {
		debug.SetGCPercent(c.gogc)
		debug.SetMemoryLimit(c.before)
		var during int64
		underTightGC(func() { during = debug.SetMemoryLimit(-1) })
		if c.low && during >= c.before {
			t.Errorf("%s: read under limit %d, want what Go held", c.name, during)
		}
		if !c.low && during != c.before {
			t.Errorf("%s: read under limit %d, want %d kept", c.name, during, c.before)
		}
		if after := debug.SetMemoryLimit(-1); after != c.before {
			t.Errorf("%s: after the read the limit is %d, want %d", c.name, after, c.before)
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
