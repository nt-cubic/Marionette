package provider

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// claudeLockAccounts signs Claude Code in to b, with a saved beside it,
// and answers the path of Claude Code's credentials.
func claudeLockAccounts(t *testing.T) (home, cred string) {
	t.Helper()
	home = claudeHome(t)
	cred = claudeSignIn(t, home, time.Now().Add(time.Hour))
	profile := filepath.Join(home, ".claude.json")
	writeFile(t, profile, map[string]any{"oauthAccount": map[string]any{"emailAddress": "a@example.com", "accountUuid": "u-a"}})
	rememberLogins(true)
	claudeWriteB(t, cred, "b")
	writeFile(t, profile, map[string]any{"oauthAccount": map[string]any{"emailAddress": "b@example.com", "accountUuid": "u-b"}})
	forgetClaudeCredential()
	rememberLogins(true)
	old := ClaudeHandedOver
	t.Cleanup(func() { ClaudeHandedOver = old })
	ClaudeHandedOver = func(string) {}
	return home, cred
}

func claudeWriteB(t *testing.T, cred, rt string) {
	t.Helper()
	writeFile(t, cred, map[string]any{"claudeAiOauth": map[string]any{"accessToken": "sk-ant-oat01-" + rt, "refreshToken": "sk-ant-ort01-" + rt,
		"expiresAt": time.Now().Add(time.Hour).UnixMilli(), "subscriptionType": "pro"}})
}

func savedAuth(t *testing.T, user string) string {
	t.Helper()
	for _, l := range readLogins() {
		if l.Agent == "claude" && l.User == user {
			return string(l.Auth)
		}
	}
	t.Fatalf("%s not saved", user)
	return ""
}

// #4532: a switch while Claude Code refreshes b's sign-in under its
// .oauth_refresh.lock waits for the refresh, and saves b as refreshed: read
// before, b's refresh token was the one the refresh spent, and b was
// signed out of magpie on its next use.
func TestClaudeSwitchWaitsForClaudeCodesRefresh(t *testing.T) {
	for _, name := range []string{"refresh", "legacy", "config"} {
		t.Run(name, func(t *testing.T) {
			home, cred := claudeLockAccounts(t)
			lock := map[string]string{
				"refresh": filepath.Join(home, ".claude", ".oauth_refresh.lock"),
				"legacy":  filepath.Join(home, ".claude.lock"),
				"config":  filepath.Join(home, ".claude.json.lock"),
			}[name]
			if err := os.Mkdir(lock, 0o755); err != nil { // Claude Code holds it
				t.Fatal(err)
			}
			done := make(chan error, 1)
			go func() { done <- SwitchLogin("claude", "a@example.com") }()
			// it waits as long as the lock is held, and touched
			for range 5 {
				select {
				case err := <-done:
					t.Fatalf("switched while Claude Code held %s: %v", filepath.Base(lock), err)
				case <-time.After(100 * time.Millisecond):
				}
				now := time.Now()
				_ = os.Chtimes(lock, now, now)
			}
			if name != "config" {
				// the refresh: a new pair written, the old refresh token spent
				claudeWriteB(t, cred, "b2")
			}
			if err := os.Remove(lock); err != nil {
				t.Fatal(err)
			}
			select {
			case err := <-done:
				if err != nil {
					t.Fatal(err)
				}
			case <-time.After(30 * time.Second):
				t.Fatal("the switch never went on after the lock was let go")
			}
			if name != "config" {
				if a := savedAuth(t, "b@example.com"); !strings.Contains(a, "sk-ant-ort01-b2") {
					t.Fatalf("b saved with the refresh token Claude Code spent: %s", a)
				}
			}
			b, _ := os.ReadFile(filepath.Join(home, ".claude.json"))
			if !strings.Contains(string(b), "a@example.com") {
				t.Fatalf("Claude Code not signed in to a: %s", b)
			}
			// every lock let go
			for _, l := range []string{".claude/.oauth_refresh.lock", ".claude.lock", ".claude.json.lock"} {
				if _, err := os.Stat(filepath.Join(home, l)); !os.IsNotExist(err) {
					t.Fatalf("%s left behind: %v", l, err)
				}
			}
		})
	}
}

// A lock its holder left (not touched for longer than Claude Code's stale
// time) is taken over, as Claude Code does; one held past the wait leaves
// the sign-in as it was and says why.
func TestClaudeSwitchStaleAndHeldLocks(t *testing.T) {
	t.Run("stale", func(t *testing.T) {
		home, _ := claudeLockAccounts(t)
		lock := filepath.Join(home, ".claude", ".oauth_refresh.lock")
		if err := os.Mkdir(lock, 0o755); err != nil {
			t.Fatal(err)
		}
		old := time.Now().Add(-2 * time.Minute)
		_ = os.Chtimes(lock, old, old)
		done := make(chan error, 1)
		go func() { done <- SwitchLogin("claude", "a@example.com") }()
		select {
		case err := <-done:
			if err != nil {
				t.Fatal(err)
			}
		case <-time.After(30 * time.Second):
			t.Fatal("a stale lock held the switch")
		}
	})
	t.Run("held", func(t *testing.T) {
		home, cred := claudeLockAccounts(t)
		w := claudeLockWait
		claudeLockWait = 300 * time.Millisecond
		t.Cleanup(func() { claudeLockWait = w })
		if err := os.Mkdir(home+"/.claude.lock", 0o755); err != nil {
			t.Fatal(err)
		}
		err := SwitchLogin("claude", "a@example.com")
		if !errors.Is(err, errClaudeLocked) {
			t.Fatalf("got %v", err)
		}
		b, _ := os.ReadFile(cred)
		if !strings.Contains(string(b), "sk-ant-ort01-b") {
			t.Fatalf("Claude Code's sign-in written over while it held its lock: %s", b)
		}
		// the first lock, taken, is let go when the second can't be
		if _, err := os.Stat(filepath.Join(home, ".claude", ".oauth_refresh.lock")); !os.IsNotExist(err) {
			t.Fatalf(".oauth_refresh.lock left behind: %v", err)
		}
	})
}
