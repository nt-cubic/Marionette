package provider

import (
	"testing"
	"time"
)

// TestLoginsListedOnceARequest: /api/providers asks an agent's accounts
// of every provider, and each list took logins.json's lock; a request
// lists an agent's once, a copy each, and a write in it lists anew.
func TestLoginsListedOnceARequest(t *testing.T) {
	signIn(t)
	loginsSeenAt.Store(0) // another test's look within 30s would skip this HOME's
	defer func(d time.Duration) { heldFor = d }(heldFor)
	heldFor = time.Hour // a loaded machine's pause isn't the bound
	release := Hold()
	defer func() { release() }()
	Logins("codex") // the first list notes the sign-in in logins.json
	first := Logins("codex")
	if len(first) != 1 || first[0].User != "me@example.com" {
		t.Fatalf("codex accounts: %+v", first)
	}
	runs := loginsRuns.Load()
	for range 20 {
		ls := Logins("codex")
		if len(ls) != 1 || ls[0].User != "me@example.com" {
			t.Fatalf("listed again as %+v", ls)
		}
		ls[0].User = "changed"
	}
	if n := loginsRuns.Load() - runs; n != 0 {
		t.Fatalf("20 lists in one request listed %d times, want 0", n)
	}
	if err := writeLogins(readLogins()); err != nil {
		t.Fatal(err)
	}
	Logins("codex")
	if n := loginsRuns.Load() - runs; n != 1 {
		t.Fatalf("after a write: listed %d times, want 1", n)
	}
	release()
	release = func() {}
	Logins("codex")
	Logins("codex")
	if n := loginsRuns.Load() - runs; n != 3 {
		t.Fatalf("outside a request: listed %d times, want 3", n)
	}
}
