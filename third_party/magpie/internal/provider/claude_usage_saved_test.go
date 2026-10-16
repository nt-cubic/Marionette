package provider

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// A saved account's card promises to run Claude Code's /usage, even when
// Claude Code itself is signed in to another account (#1630, hhh2210).
func TestRefreshSavedClaudeUsage(t *testing.T) {
	claudeThree(t)
	var text atomic.Value
	text.Store("Current session: 17% used\nCurrent week (all models): 38% used")
	runs := fakeClaudeUsage(t, &text, nil)
	var expectSaved atomic.Bool
	UsageClaudeVia(func(_ context.Context, dir string) (string, error) {
		runs.Add(1)
		if expectSaved.Load() {
			if dir != claudeAccountDir("max@x.com") {
				return "", errors.New("/usage ran in another account's directory")
			}
			c, ok := readClaudeDir(dir)
			if !ok || c.OAuth.AccessToken != "tok-max@x.com" {
				return "", errors.New("/usage received another account's sign-in")
			}
		}
		return text.Load().(string), nil
	})
	ctx := context.Background()
	before := LoginUsage(ctx, "claude")
	if before["max@x.com"].Error != errClaudeSaved.Error() {
		t.Fatalf("unasked saved account: %+v", before["max@x.com"])
	}
	// the Usage page opened, `magpie quota`: the user asking, but not for
	// this account, and each saved account would start a Claude Code
	beforeRuns := runs.Load()
	AskClaudeUsage()
	if q := LoginUsage(Asked(ctx), "claude")["max@x.com"]; q.Error != errClaudeSaved.Error() || runs.Load() != beforeRuns {
		t.Fatalf("an ask for every card ran a saved account: %+v, %d runs", q, runs.Load()-beforeRuns)
	}
	beforeRuns = runs.Load()
	expectSaved.Store(true)
	RefreshUsage(ctx, "claude", "max@x.com")
	after := LoginUsage(ctx, "claude")
	q := after["max@x.com"]
	if q.Error != "" || len(q.Windows) != 2 || q.Windows[0].Used != 17 || q.Windows[1].Used != 38 || q.ReadAt == nil {
		t.Fatalf("saved account's refresh read no allowance: %+v", q)
	}
	if runs.Load() != beforeRuns+1 {
		t.Fatalf("refresh ran Claude Code %d times, want once", runs.Load()-beforeRuns)
	}
	if after["team@x.com · Acme"].Error != before["team@x.com · Acme"].Error {
		t.Fatal("refresh changed another saved account's card")
	}
	active := ""
	for _, l := range Logins("claude") {
		if l.Active {
			active = l.User
		}
	}
	if active != "pro@x.com" {
		t.Fatalf("refresh changed Claude Code's own account to %q", active)
	}

	// Rapid clicks still respect the per-account floor. A later explicit
	// refresh reads anew, while a background read keeps what was read.
	RefreshUsage(ctx, "claude", "max@x.com")
	if runs.Load() != beforeRuns+1 {
		t.Fatal("rapid refresh bypassed the per-account floor")
	}
	claudeUsage.Lock()
	e := claudeUsage.m["max@x.com"]
	e.tried = e.tried.Add(-time.Hour)
	claudeUsage.m["max@x.com"] = e
	claudeUsage.Unlock()
	if ws, err := claudeWindows(ctx, "max@x.com", false); err != nil || len(ws) != 2 || runs.Load() != beforeRuns+1 {
		t.Fatalf("background read ran a saved account: %v %+v", err, ws)
	}
	text.Store("Current session: 21% used\nCurrent week (all models): 40% used")
	RefreshUsage(ctx, "claude", "max@x.com")
	q = LoginUsage(ctx, "claude")["max@x.com"]
	if q.Error != "" || len(q.Windows) != 2 || q.Windows[0].Used != 21 || runs.Load() != beforeRuns+2 {
		t.Fatalf("later refresh kept an old reading: %+v", q)
	}
}

// Claude Code run in a saved account's directory refreshes the sign-in it
// keeps there when it is due, spending the old refresh token. magpie keeps
// the new one at once, and doesn't hand the account to Claude Code itself
// while that run may still be refreshing it: Claude Code's own copy would
// be the spent one.
func TestSavedClaudeUsageKeepsRotatedSignIn(t *testing.T) {
	claudeThree(t)
	home := os.Getenv("HOME")
	var text atomic.Value
	text.Store("Current session: 17% used")
	fakeClaudeUsage(t, &text, nil)
	running, release := make(chan string, 1), make(chan struct{})
	UsageClaudeVia(func(_ context.Context, dir string) (string, error) {
		if dir == "" {
			return text.Load().(string), nil
		}
		running <- dir
		<-release
		// as Claude Code refreshes: the new sign-in written where it
		// keeps it for this directory
		writeFile(t, filepath.Join(dir, ".credentials.json"), map[string]any{"claudeAiOauth": map[string]any{
			"accessToken": "tok-rotated", "refreshToken": "r-rotated",
			"expiresAt": time.Now().Add(8 * time.Hour).UnixMilli(), "subscriptionType": "max"}})
		return text.Load().(string), nil
	})
	var freed sync.Once
	free := func() { freed.Do(func() { close(release) }) }
	t.Cleanup(free) // a failing test doesn't leave the run waiting
	refreshed := make(chan struct{})
	go func() {
		defer close(refreshed)
		RefreshUsage(context.Background(), "claude", "max@x.com")
	}()
	var dir string
	select {
	case dir = <-running:
	case <-refreshed:
		t.Fatal("the card's refresh ran no /usage in the account's directory")
	}
	switched := make(chan error, 1)
	go func() { switched <- SwitchLogin("claude", "max@x.com") }()
	select {
	case err := <-switched:
		t.Fatalf("switched to the account while its /usage ran (%v)", err)
	case <-time.After(300 * time.Millisecond):
	}
	free()
	<-refreshed
	if err := <-switched; err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(home, ".claude", ".credentials.json"))
	if err != nil || !strings.Contains(string(b), "r-rotated") {
		t.Fatalf("Claude Code was handed the spent sign-in: %s %v", b, err)
	}
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatalf("the account's directory still holds its sign-in after the switch: %v", err)
	}
}

// Without a switch, the rotated sign-in is the saved account's in
// logins.json as soon as the run ends, not when the account is next used.
func TestSavedClaudeUsageSavesRotatedSignIn(t *testing.T) {
	claudeThree(t)
	var text atomic.Value
	text.Store("Current session: 17% used")
	fakeClaudeUsage(t, &text, nil)
	UsageClaudeVia(func(_ context.Context, dir string) (string, error) {
		if dir != "" {
			writeFile(t, filepath.Join(dir, ".credentials.json"), map[string]any{"claudeAiOauth": map[string]any{
				"accessToken": "tok-rotated", "refreshToken": "r-rotated",
				"expiresAt": time.Now().Add(8 * time.Hour).UnixMilli(), "subscriptionType": "max"}})
		}
		return text.Load().(string), nil
	})
	RefreshUsage(context.Background(), "claude", "max@x.com")
	loginsMu.Lock()
	ls := readLogins()
	loginsMu.Unlock()
	for _, l := range ls {
		if l.Agent == "claude" && l.User == "max@x.com" {
			if c, ok := parseClaudeCredentials(l.Auth); !ok || c.OAuth.RefreshToken != "r-rotated" {
				t.Fatalf("logins.json kept the spent sign-in: %s", l.Auth)
			}
			return
		}
	}
	t.Fatal("max@x.com isn't saved")
}
