package plugin

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

// plugin-updates.json is read as no updates when it doesn't parse, and the
// next check wrote that over it: Updated, the plugins magpie updated by
// itself, can't be found again (#2263). What was there is copied aside
// first.
func TestUnreadUpdatesKeptAside(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	if err := os.MkdirAll(filepath.Dir(updatesPath()), 0o700); err != nil {
		t.Fatal(err)
	}
	was := `{"updated":[{"package":"opencode-gemini-auth","from":"0.1.2","to":"0.2`
	if err := os.WriteFile(updatesPath(), []byte(was), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := CheckUpdates(context.Background()); err != nil {
		t.Fatal(err)
	}
	bad, _ := filepath.Glob(updatesPath() + ".bad-*")
	if len(bad) != 1 {
		t.Fatalf("kept aside: %v", bad)
	}
	if b, _ := os.ReadFile(bad[0]); string(b) != was {
		t.Fatalf("kept aside as %q, want %q", b, was)
	}
	if PendingUpdates().Checked.IsZero() {
		t.Fatal("the check wasn't written")
	}
}
