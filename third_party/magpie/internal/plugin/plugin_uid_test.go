package plugin

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// Two accounts of one name that the plugin tells apart by a uid (WorkBuddy's
// nickname and uid) are both kept: the second took the first one's place
// (#413). The same uid again replaces its own sign-in.
func TestPluginSameNameOtherUID(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	signIn := func(team string) Saved {
		t.Helper()
		a, err := Authorize(ctx, "fakeco", 1, map[string]string{"where": "work", "team": team}, NewAccount)
		if err != nil {
			t.Fatal(err)
		}
		s, err := Finish(ctx, a.Session, "good")
		if err != nil {
			t.Fatal(err)
		}
		return s
	}
	saved := func() map[string]map[string]any {
		t.Helper()
		var m map[string]map[string]any
		b, _ := os.ReadFile(AuthPath())
		json.Unmarshal(b, &m)
		return m
	}
	first, second := signIn("pat/u1"), signIn("pat/u2")
	if first.Account == second.Account {
		t.Fatalf("the second took the first's place: %+v %+v", first, second)
	}
	if m := saved(); len(m) != 2 || m[first.Account]["uid"] != "u1" || m[second.Account]["uid"] != "u2" {
		t.Fatalf("saved %v", m)
	}
	if again := signIn("pat/u1"); again != first {
		t.Fatalf("u1 again as %+v, not %+v", again, first)
	}
	if m := saved(); len(m) != 2 {
		t.Fatalf("after u1 again %v", m)
	}
}

// A sign-in again to an account an earlier version of its plugin named
// another way replaces that account's record (#4473). officeace-auth
// 0.1.14 to 0.1.17 named an account by its masked phone and kept the
// Huawei domain id as its uid; the CodeArts plugin's OfficeAce card names
// a new sign-in by the domain id, uid the same. The record is joined by
// that uid, since the new one is named by it, and keeps the account id the
// user's name, proxy and gateway keys are kept by. Two accounts that only
// share a uid, as Kiro's two Google sign-ins share "google", stay apart.
func TestPluginReSignInJoinsAnOldIDShape(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	const domain = "01a1642ad2ae7f93bf6387ad2fc5f09e"
	// as officeace-auth 0.1.14's callback and credential.mjs left it
	old := map[string]any{
		"fakeco": map[string]any{
			"type":      "oauth",
			"refresh":   "eyJ.dead.refresh",
			"access":    `{"access":"HSTA","securityToken":"hQpj","refreshToken":"eyJ.dead.refresh","accountId":"` + domain + `","user_phone":"189****8051"}`,
			"expires":   1792841892266,
			"accountId": "189****8051",
			"uid":       domain,
		},
	}
	b, _ := json.Marshal(old)
	if err := os.MkdirAll(filepath.Dir(AuthPath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(AuthPath(), b, 0o600); err != nil {
		t.Fatal(err)
	}
	signIn := func(team string) Saved {
		t.Helper()
		a, err := Authorize(ctx, "fakeco", 1, map[string]string{"where": "work", "team": team}, NewAccount)
		if err != nil {
			t.Fatal(err)
		}
		s, err := Finish(ctx, a.Session, "good")
		if err != nil {
			t.Fatal(err)
		}
		return s
	}
	saved := func() map[string]map[string]any {
		t.Helper()
		var m map[string]map[string]any
		b, _ := os.ReadFile(AuthPath())
		json.Unmarshal(b, &m)
		return m
	}
	if s := signIn("=" + domain); s.Account != "fakeco" {
		t.Fatalf("the sign-in again went to %q, beside the old record: %v", s.Account, saved())
	}
	m := saved()
	if len(m) != 1 {
		t.Fatalf("saved %v", m)
	}
	if r := m["fakeco"]; r["refresh"] != "r-="+domain || r["accountId"] != "189****8051" || r["uid"] != domain {
		t.Fatalf("the old record wasn't replaced as that account: %v", r)
	}

	// Kiro keeps how an account was signed in to as its uid: two Google
	// sign-ins share "google" and are two accounts.
	a, b2 := signIn("ann/google"), signIn("bob/google")
	if a.Account == b2.Account || len(saved()) != 3 {
		t.Fatalf("two accounts of one uid were joined: %+v %+v %v", a, b2, saved())
	}
}
