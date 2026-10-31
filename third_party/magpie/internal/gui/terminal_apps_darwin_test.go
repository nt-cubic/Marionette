//go:build darwin && cgo

package gui

import (
	"context"
	"slices"
	"strings"
	"testing"
)

func TestDiscoverTerminals(t *testing.T) {
	found, err := discoverTerminals()
	if err != nil {
		t.Fatal(err)
	}
	terminalFound := false
	for _, app := range found.Apps {
		if app.ID == "com.apple.TextEdit" {
			t.Fatal("a text editor was offered as a session terminal")
		}
		if app.ID == terminalBundleID && app.Path != "" {
			terminalFound = true
		}
	}
	if !terminalFound {
		t.Fatalf("Terminal.app missing from .command handlers: %+v", found.Apps)
	}
}

// StringKe (#4565): Rex claims no .command file, so Launch Services never
// lists it; it is offered when its CLI is found, and opened through it.
func TestDiscoverTerminalsOffersRex(t *testing.T) {
	was := rexFound
	t.Cleanup(func() { rexFound = was })
	rexFound = func() string { return "/opt/rex/bin/rex" }
	found, err := discoverTerminals()
	if err != nil {
		t.Fatal(err)
	}
	rex := slices.IndexFunc(found.Apps, func(a terminalApp) bool { return a.ID == rexBundleID })
	if rex < 0 || found.Apps[rex].Path != "/opt/rex/bin/rex" || found.Apps[rex].Name != "Rex" {
		t.Fatalf("Rex not offered with its CLI found: %+v", found.Apps)
	}
	var ran [][]string
	wasRun, wasShow := rexRun, rexShow
	t.Cleanup(func() { rexRun, rexShow = wasRun, wasShow })
	rexRun = func(_ context.Context, bin string, args ...string) ([]byte, error) {
		ran = append(ran, append([]string{bin}, args...))
		return nil, nil
	}
	rexShow = func() {}
	if err := openTerminal("cd '/tmp' && claude --resume x", "Fix the build", rexBundleID); err != nil {
		t.Fatal(err)
	}
	if len(ran) != 1 || ran[0][0] != "/opt/rex/bin/rex" || ran[0][1] != "run" || !slices.Contains(ran[0], "Fix the build") {
		t.Fatalf("rex ran %q, want one rex run labelled with the session's title", ran)
	}
	rexFound = func() string { return "" }
	if err := openTerminal("claude", "", rexBundleID); err == nil || !strings.Contains(err.Error(), "no longer available") {
		t.Fatalf("Rex chosen with no CLI found: %v, want it said to be unavailable", err)
	}
	if len(ran) != 1 {
		t.Fatalf("rex ran again with none found: %q", ran)
	}
}
