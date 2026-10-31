package gui

import (
	"testing"

	"github.com/yetone/magpie/internal/tune"
)

// TestCodexAsyncAt: the advice as a share of the window Codex's background
// compaction starts at, rounded down; none where it would start no sooner
// than the foreground limit (the lesser of the limit now and 90% of the
// window) or the window isn't known. Timed compactions take the cheapest
// window, since one in the background keeps no one waiting.
func TestCodexAsyncAt(t *testing.T) {
	for _, c := range []struct {
		name       string
		r          *tune.Compaction
		pct, value int
		ok         bool
	}{
		{"herman_hong's", &tune.Compaction{Current: 384000, Window: 400000, Best: 120000}, 30, 120000, true},
		{"rounded down", &tune.Compaction{Current: 384000, Window: 400000, Best: 123000}, 30, 120000, true},
		{"timed takes the cheapest", &tune.Compaction{Current: 384000, Window: 400000, Best: 200000, Cheapest: 100000, Facts: tune.Facts{Took: 3}}, 25, 100000, true},
		{"untimed keeps the advice", &tune.Compaction{Current: 384000, Window: 400000, Best: 200000, Cheapest: 100000}, 50, 200000, true},
		{"not before the foreground", &tune.Compaction{Current: 120000, Window: 400000, Best: 120000}, 30, 120000, false},
		{"not past 90%", &tune.Compaction{Window: 400000, Best: 380000}, 95, 380000, false},
		{"at least 1%", &tune.Compaction{Current: 900000, Window: 1000000, Best: 5000}, 1, 10000, true},
		{"no window", &tune.Compaction{Current: 384000, Best: 120000}, 0, 0, false},
		{"none", nil, 0, 0, false},
	} {
		pct, v, ok := codexAsyncAt(c.r)
		if ok != c.ok || (ok && (pct != c.pct || v != c.value)) || (!ok && c.pct == 0 && (pct != 0 || v != 0)) {
			t.Errorf("%s: %d%% %d %v, want %d%% %d %v", c.name, pct, v, ok, c.pct, c.value, c.ok)
		}
	}
}
