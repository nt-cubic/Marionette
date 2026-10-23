package sessions

import "testing"

// The overview's stats and the sessions that tell its gateway sessions
// apart come of one pass over every agent's files (#2776): it took a second
// pass, through List(All), on every refresh. The keys are List's.
func TestStatsAndNativeOnePass(t *testing.T) {
	setup(t)
	want := map[string]bool{}
	for _, s := range List(All) {
		want[s.Agent+"|"+s.ID] = true
	}
	passes := 0
	filesHook = func() { passes++ }
	t.Cleanup(func() { filesHook = nil })
	st, native := StatsAndNative(0)
	if passes != 1 {
		t.Fatalf("%d passes over the files", passes)
	}
	if len(st.Sessions) == 0 {
		t.Fatalf("no stats: %+v", st)
	}
	if len(native) != len(want) || len(want) < 2 {
		t.Fatalf("native %v, List's %v", native, want)
	}
	for k := range want {
		if !native[k] {
			t.Fatalf("native %v, List's %v", native, want)
		}
	}
}
