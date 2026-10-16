package update

import "testing"

// A Mac restart by itself with no window up opens the app as a login launch
// does, its menu bar icon only (#2096); one with the window opens it.
func TestRelaunchOpen(t *testing.T) {
	if got, want := relaunchOpen("/Applications/magpie.app", true), `open "/Applications/magpie.app"`; got != want {
		t.Errorf("window: %s, want %s", got, want)
	}
	if got, want := relaunchOpen("/Applications/magpie.app", false), `open "/Applications/magpie.app" --args tray`; got != want {
		t.Errorf("no window: %s, want %s", got, want)
	}
}
