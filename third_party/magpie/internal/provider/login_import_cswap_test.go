package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
)

// StringKe (#4532): moving from claude-swap to magpie took signing in to
// every account again, since `magpie accounts import claude` didn't read
// cswap's export. testdata/cswap_0.29.0_export.json is what cswap 0.29.0's
// own `cswap export` wrote (in a sandbox HOME on Linux, with made-up
// tokens): a Max account and a Team seat named "work". Each comes in
// with its sign-in as the file has it, its account and organization, and
// "work" as its name in magpie; Anthropic is asked nothing.
func TestImportCswapExport(t *testing.T) {
	claudeHome(t)
	asked := false
	fake := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		asked = true
		w.WriteHeader(500)
	}))
	defer fake.Close()
	oldBase := claudeBase
	t.Cleanup(func() { claudeBase = oldBase })
	claudeBase = fake.URL

	b, err := os.ReadFile("testdata/cswap_0.29.0_export.json")
	if err != nil {
		t.Fatal(err)
	}
	rs, err := ImportLogins(context.Background(), "claude", []string{string(b)})
	if err != nil {
		t.Fatal(err)
	}
	if got := importStatuses(rs); got != "max@x.com:added,team@x.com:added" || asked {
		t.Fatalf("got %s %+v asked Anthropic: %v", got, rs, asked)
	}
	byToken := map[string]savedLogin{}
	for _, l := range readLogins() {
		byToken[loginRefreshToken(l)] = l
	}
	for _, c := range []struct{ token, email, account, org, orgName, name string }{
		{"sk-ant-ort01-ref-max", "max@x.com", "acct-max", "org-max", "max@x.com's Organization", ""},
		{"sk-ant-ort01-ref-team", "team@x.com", "acct-team", "org-acme", "Acme", "work"},
	} {
		l, ok := byToken[c.token]
		if !ok {
			t.Fatalf("%s wasn't kept: %v", c.token, byToken)
		}
		email, org := claudeWho(l.Profile)
		id, _ := claudeSessionIdentity(l.Profile)
		var p map[string]any
		_ = json.Unmarshal(l.Profile, &p)
		cr, _ := parseClaudeCredentials(l.Auth)
		if email != c.email || org != c.org || id.AccountID != c.account || p["organizationName"] != c.orgName ||
			cr.OAuth.AccessToken != strings.Replace(c.token, "ort01-ref", "oat01-acc", 1) || len(cr.OAuth.Scopes) != 3 {
			t.Fatalf("%s: %s %s %+v %v %+v", c.token, email, org, id, p, cr.OAuth)
		}
		if got := AccountNameOf("claude", l.User); got != c.name {
			t.Fatalf("%s is named %q, not %q", l.User, got, c.name)
		}
	}

	// again: both are magpie's already, and a name the user gave since
	// is kept
	if err := SetAccountName("claude", "team@x.com", "mine"); err != nil {
		t.Fatal(err)
	}
	rs, err = ImportLogins(context.Background(), "claude", []string{string(b)})
	if err != nil {
		t.Fatal(err)
	}
	if got := importStatuses(rs); got != "max@x.com:exists,team@x.com:exists" || AccountNameOf("claude", "team@x.com") != "mine" {
		t.Fatalf("again: %s, named %q", got, AccountNameOf("claude", "team@x.com"))
	}
}

// What cswap's own import refuses, magpie refuses too: an encrypted
// export, another format. A cswap API-key account is a key, a cswap file
// isn't Codex's, and a name another account has already isn't given.
func TestImportCswapRefusals(t *testing.T) {
	claudeHome(t)
	b, err := os.ReadFile("testdata/cswap_0.29.0_export.json")
	if err != nil {
		t.Fatal(err)
	}
	with := func(f func(env map[string]any)) string {
		var env map[string]any
		if err := json.Unmarshal(b, &env); err != nil {
			t.Fatal(err)
		}
		f(env)
		out, _ := json.Marshal(env)
		return string(out)
	}
	for _, c := range []struct {
		name, file, want string
	}{
		{"encrypted", with(func(env map[string]any) { env["encrypted"] = true }), "encrypted claude-swap export"},
		{"format 2", with(func(env map[string]any) { env["version"] = 2 }), "format 2"},
	} {
		if _, err := ImportLogins(context.Background(), "claude", []string{c.file}); err == nil || !strings.Contains(err.Error(), c.want) {
			t.Fatalf("%s: %v", c.name, err)
		}
	}

	key := with(func(env map[string]any) {
		a := env["accounts"].([]any)
		k := a[0].(map[string]any)
		k["credentials"], k["kind"] = "sk-ant-api03-key", "api_key"
		env["accounts"] = a[:1]
	})
	rs, err := ImportLogins(context.Background(), "claude", []string{key})
	if err != nil || len(rs) != 1 || rs[0].Status != "failed" || !strings.Contains(rs[0].Error, "API key") {
		t.Fatalf("api key: %+v %v", rs, err)
	}
	rs, err = ImportLogins(context.Background(), "codex", []string{string(b)})
	if err != nil || len(rs) != 2 || rs[0].Status != "failed" || !strings.Contains(rs[0].Error, "not Codex's") {
		t.Fatalf("codex: %+v %v", rs, err)
	}
	if len(readLogins()) != 0 {
		t.Fatalf("saved: %+v", readLogins())
	}

	// "work" is another account's name already: the team seat comes in
	// without it
	loginsMu.Lock()
	err = writeLogins(upsertLogin(readLogins(), savedLogin{Agent: "claude", User: "pro@x.com", Auth: []byte(`{"claudeAiOauth":{"accessToken":"a","refreshToken":"r-pro"}}`)}))
	loginsMu.Unlock()
	if err != nil {
		t.Fatal(err)
	}
	if err := SetAccountName("claude", "pro@x.com", "Work"); err != nil {
		t.Fatal(err)
	}
	if _, err := ImportLogins(context.Background(), "claude", []string{string(b)}); err != nil {
		t.Fatal(err)
	}
	if got := AccountNameOf("claude", "team@x.com"); got != "" {
		t.Fatalf("the team seat took pro's name: %q", got)
	}
	if got := AccountNameOf("claude", "pro@x.com"); got != "Work" {
		t.Fatalf("pro's name: %q", got)
	}
}
