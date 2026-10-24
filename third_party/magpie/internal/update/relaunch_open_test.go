package update

import (
	"strings"
	"testing"
)

// A Mac restart by itself with no window up opens the app as a login launch
// does, its menu bar icon only (#2096); one with the window opens it. Both
// ask for a new instance (-n), so LaunchServices launches the bundle rather
// than send a reopen to the magpie that has just exited (Linka's -600).
func TestRelaunchOpen(t *testing.T) {
	if got, want := relaunchOpen("/Applications/magpie.app", true), `open -n "/Applications/magpie.app"`; got != want {
		t.Errorf("window: %s, want %s", got, want)
	}
	if got, want := relaunchOpen("/Applications/magpie.app", false), `open -n "/Applications/magpie.app" --args tray`; got != want {
		t.Errorf("no window: %s, want %s", got, want)
	}
}

// A restart that hands the gateway over opens the new version while this
// one runs: a new instance (-n), which LaunchServices would otherwise send
// to the one running.
func TestOpenBeside(t *testing.T) {
	if got, want := openBeside("/Applications/magpie.app", true), `open -n "/Applications/magpie.app"`; got != want {
		t.Errorf("window: %s, want %s", got, want)
	}
	if got, want := openBeside("/Applications/magpie.app", false), `open -n "/Applications/magpie.app" --args tray`; got != want {
		t.Errorf("no window: %s, want %s", got, want)
	}
}

// The new version a handover starts isn't told to wait for this one to
// exit (it would wait while this one waits for it), and gets the args
// RelaunchBinary gives.
func TestStartBesideWaitsForNothing(t *testing.T) {
	t.Setenv("MAGPIE_REPLACES", "")
	var gotArgs, gotEnv []string
	stubLaunch(t, func(exe string, args, env []string) (<-chan struct{}, func(), error) {
		gotArgs, gotEnv = args, env
		return nil, nil, nil
	})
	if err := StartBeside("/opt/magpie", true, "routing"); err != nil {
		t.Fatal(err)
	}
	if strings.Join(gotArgs, " ") != "gui routing" {
		t.Errorf("args %q", gotArgs)
	}
	for _, e := range gotEnv {
		if strings.HasPrefix(e, "MAGPIE_REPLACES=") && e != "MAGPIE_REPLACES=" {
			t.Errorf("told to wait: %s", e)
		}
	}
}
