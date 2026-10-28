package provider

import (
	"testing"
	"time"
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
