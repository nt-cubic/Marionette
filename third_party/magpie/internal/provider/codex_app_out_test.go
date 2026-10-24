package provider

import (
	"context"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/proc"
)

// A reply that says the account Codex is signed in to is out has its
// allowance read again at once, not at the next look five minutes on
// (MasterYi on Discord: the Codex app greyed Send meanwhile): once per
// codexRecheckEvery, for the active account alone, and only in a magpie
// that keeps Codex on an account with room.
func TestCodexMayBeUsedUpReadsAtOnce(t *testing.T) {
	signIn(t) // me@example.com, active
	rememberLogins(true)
	reads := make(chan string, 8)
	oldRead, oldEvery := codexReadNow, codexRecheckEvery
	codexReadNow = func(user string) { reads <- user }
	t.Cleanup(func() {
		codexReadNow, codexRecheckEvery = oldRead, oldEvery
		codexLiveChecks.Store(0)
		codexRecheck.Lock()
		codexRecheck.at = time.Time{}
		codexRecheck.Unlock()
	})
	none := func(when string) {
		t.Helper()
		select {
		case u := <-reads:
			t.Fatalf("%s: read %q", when, u)
		case <-time.After(50 * time.Millisecond):
		}
	}
	read := func(when string) {
		t.Helper()
		select {
		case u := <-reads:
			if u != "me@example.com" {
				t.Fatalf("%s: read %q", when, u)
			}
		case <-time.After(10 * time.Second):
			t.Fatalf("%s: not read", when)
		}
	}
	full := []CodexLimit{{Used: 100, Span: 5 * time.Hour}}

	NoteCodexLimits("codex", "me@example.com", full)
	none("live checks off")

	codexLiveChecks.Store(1)
	NoteCodexLimits("codex", "me@example.com", []CodexLimit{{Used: 99, Span: 5 * time.Hour}})
	none("a window with room")
	NoteCodexLimits("codex", "other@example.com", full)
	none("an account Codex isn't on")
	NoteCodexLimits("codex", "me@example.com", full)
	read("a window full")
	CodexMayBeUsedUp("me@example.com")
	none("again at once")

	codexRecheckEvery = 0
	CodexMayBeUsedUp("ME@example.com")
	read("usage_limit_reached")
}

// The Codex app open when its account ran out is kept till it is opened
// again (its app-server's pid changes) or the account has room, and may
// be reopened once for each time the account runs out.
func TestCodexAppOutEpisode(t *testing.T) {
	signIn(t)
	rememberLogins(true)
	app := new(int)
	old := listProcesses
	t.Cleanup(func() {
		listProcesses = old
		noteCodexAppOut(false)
	})
	listProcesses = func(context.Context) ([]proc.Process, error) {
		ps := []proc.Process{{PID: 7, Args: "codex"}}
		if *app > 0 {
			ps = append(ps, proc.Process{PID: *app, Args: "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex app-server"})
		}
		return ps, nil
	}
	recheck := func() { codexAppOut.Lock(); codexAppOut.checked = time.Time{}; codexAppOut.Unlock() }

	noteCodexAppOut(true)
	if got := CodexAppOut(); got != "" || CodexAppReopenOnce() {
		t.Fatalf("no app open: %q", got)
	}
	*app = 60
	noteCodexAppOut(true)
	if got := CodexAppOut(); got != "me@example.com" {
		t.Fatalf("app open as it ran out: %q", got)
	}
	if !CodexAppReopenOnce() || CodexAppReopenOnce() {
		t.Fatal("reopened other than once")
	}
	recheck()
	if got := CodexAppOut(); got != "me@example.com" {
		t.Fatalf("still the same app: %q", got)
	}
	*app = 61
	recheck()
	if got := CodexAppOut(); got != "" {
		t.Fatalf("opened again: %q", got)
	}

	noteCodexAppOut(true) // out again: a new time, reopened once more
	if CodexAppOut() == "" || !CodexAppReopenOnce() {
		t.Fatal("out again: not kept")
	}
	noteCodexAppOut(false)
	if got := CodexAppOut(); got != "" {
		t.Fatalf("room again: %q", got)
	}
}
