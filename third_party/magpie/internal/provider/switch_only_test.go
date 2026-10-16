package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// #1907 (foobra): Codex Desktop and the CLI on their own ChatGPT
// subscription, two accounts, no gateway in between. Switch-only is kept on
// Codex's subscription; with it on, the account Codex is signed in to
// running out still signs Codex in to the next one with room, in its own
// auth.json, and back once the first has room again. Claude Code's
// subscription has no such setting. A providers.json that can't be read
// says switch-only: the user's choice is unknown, and failover through the
// gateway, magpie's own idea, isn't taken up on it.
func TestCodexSwitchOnlyStillSwitchesTheSignIn(t *testing.T) {
	home := signIn(t) // me@example.com, acct-1
	rememberLogins(true)
	codexSignIn(t, home, "spare@example.com", "r-spare")
	rememberLogins(true)
	codexSignIn(t, home, "me@example.com", "r-1")
	rememberLogins(true)
	used := map[string]float64{"acct-me@example.com": 100, "acct-spare@example.com": 20}
	fake := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]any{"plan_type": "plus", "rate_limit": map[string]any{
			"primary_window": map[string]any{"used_percent": used[r.Header.Get("chatgpt-account-id")], "limit_window_seconds": 18000}}})
	}))
	defer fake.Close()
	old := CodexBase
	CodexBase = fake.URL + "/backend-api/codex"
	t.Cleanup(func() { CodexBase = old })
	switched := func() string {
		t.Helper()
		loginUsageCache.Lock()
		loginUsageCache.m = nil
		loginUsageCache.Unlock()
		to, err := SwitchWhenSpent(context.Background(), "codex")
		if err != nil {
			t.Fatal(err)
		}
		return to
	}
	signedIn := func() string {
		t.Helper()
		var live codexAuth
		readJSON(filepath.Join(home, ".codex", "auth.json"), &live)
		return live.Tokens.RefreshToken
	}
	if err := SetLoginOn("codex", "spare@example.com", true); err != nil {
		t.Fatal(err)
	}

	if SwitchOnly("codex") {
		t.Fatal("switch-only before the user set it")
	}
	if err := SetSwitchOnly("codex", true); err != nil {
		t.Fatal(err)
	}
	if !SwitchOnly("codex") {
		t.Fatal("switch-only not kept")
	}
	p, err := Find("codex")
	if err != nil || !p.SwitchOnly {
		t.Fatalf("Find: %+v %v", p, err)
	}
	if err := SetSwitchOnly("claude", true); err == nil {
		t.Error("switch-only set on Claude Code's subscription")
	}

	// out: on to the spare, in Codex's own auth.json
	if to := switched(); to != "spare@example.com" || signedIn() != "r-spare" {
		t.Fatalf("switched to %q, Codex signed in with %q", to, signedIn())
	}
	// the first has room again: back to it
	used["acct-me@example.com"] = 10
	if to := switched(); !strings.HasPrefix(to, "me@example.com") || signedIn() != "r-1" {
		t.Fatalf("back to %q, Codex signed in with %q", to, signedIn())
	}
	if !SwitchOnly("codex") {
		t.Fatal("switch-only lost on a switch")
	}

	if err := SetSwitchOnly("codex", false); err != nil || SwitchOnly("codex") {
		t.Fatalf("off: %v, still %v", err, SwitchOnly("codex"))
	}
	// neither the file nor its last good generation reads
	for _, f := range []string{Path(), Path() + ".bak"} {
		if err := os.WriteFile(f, []byte("{not json"), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	if !SwitchOnly("codex") {
		t.Error("an unreadable providers.json read as switch-only off")
	}
}
