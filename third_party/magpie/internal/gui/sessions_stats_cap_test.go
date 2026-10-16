package gui

import "testing"

// ?days is any number a caller sends, and each distinct value kept a whole
// sessions.Stats in statsMemo for good: 64 values, 64 snapshots. The memo
// keeps a few, the least recently asked let go, and the newest one stays.
func TestStatsMemoKeepsAFewDays(t *testing.T) {
	forgetStats()
	t.Cleanup(forgetStats)
	for d := 1; d <= 64; d++ {
		statsFor(d)
	}
	statsMemo.Lock()
	n, newest := len(statsMemo.m), statsMemo.m[64] != nil
	statsMemo.Unlock()
	if n > 8 {
		t.Fatalf("after 64 distinct ?days values the memo holds %d, want at most 8", n)
	}
	if !newest {
		t.Fatal("the value asked last was let go")
	}
}

// The range the page keeps asking for (the one it shows) is the one kept: a
// range asked again is newer than one asked once before it, so a run of new
// numbers lets go of the others first.
func TestStatsMemoKeepsTheRangeAskedAgain(t *testing.T) {
	forgetStats()
	t.Cleanup(forgetStats)
	for d := 1; d <= statsMemoMost; d++ {
		statsFor(d)
	}
	statsFor(1)
	statsFor(100)
	statsMemo.Lock()
	kept1, kept2 := statsMemo.m[1] != nil, statsMemo.m[2] != nil
	statsMemo.Unlock()
	if !kept1 {
		t.Fatal("the range asked again was let go before one asked once")
	}
	if kept2 {
		t.Fatal("the least recently asked range is still kept past the cap")
	}
}
