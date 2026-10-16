package davsync

import (
	"os"
	"path/filepath"
	"testing"
)

// sync-state.json is read as no state when it doesn't parse, and the next
// save wrote that over it, a restore's Undo and the Blind guard with it
// (#2263): what was there is copied aside first.
func TestUnreadSyncStateKeptAside(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	p := path("sync-state.json")
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		t.Fatal(err)
	}
	was := `{"key":"k","undo":"/somewhere`
	if err := os.WriteFile(p, []byte(was), 0o600); err != nil {
		t.Fatal(err)
	}
	saveState(state{Key: "k"})
	bad, _ := filepath.Glob(p + ".bad-*")
	if len(bad) != 1 {
		t.Fatalf("kept aside: %v", bad)
	}
	if b, _ := os.ReadFile(bad[0]); string(b) != was {
		t.Fatalf("kept aside as %q, want %q", b, was)
	}
	if loadState().Key != "k" {
		t.Fatal("the new state wasn't written")
	}
}
