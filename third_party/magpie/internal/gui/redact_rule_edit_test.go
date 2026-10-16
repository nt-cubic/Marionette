package gui

import (
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

// A masking rule is changed in place (nf1qaz2w on Discord: rules could only
// be removed): its name, prefix or pattern change where it stands, and what
// the gateway masks changes with it. A pattern that doesn't compile, a rule
// left with nothing to match, one already listed and a rule that is no
// longer there are refused with the reason, every rule kept as it was.
func TestRedactRuleEdit(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	srv := Handler(nil, nil)
	post := func(path, body string) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		srv.ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		return rec
	}
	if rec := post("/api/settings/redact-rules", `{"rules":[{"kind":"GW_KEY","prefix":"oc_sk_"},{"kind":"NS","regex":"ns-[0-9a-f]{12}"},{"kind":"LAST","prefix":"zz_tok_"}]}`); rec.Code != http.StatusOK {
		t.Fatalf("rules not set: %d %s", rec.Code, rec.Body)
	}
	masks := func(s string) bool {
		st := settings.Load()
		_, n := redact.Mask(s, redact.Options{Secrets: true, Rules: st.RedactRules})
		return n > 0
	}
	// each check a value of its own: one masked once is masked again
	// wherever it turns up (redact's known values)
	if !masks("key ns-0123456789ab here") || masks("key ns_00000000000a here") {
		t.Fatal("the rule as set masks the wrong thing")
	}

	rec := post("/api/settings/redact-rule", `{"at":1,"was":{"kind":"NS","regex":"ns-[0-9a-f]{12}"},"rule":{"kind":"ns key","regex":" ns_[0-9a-f]{12} "}}`)
	want := []redact.Rule{{Kind: "GW_KEY", Prefix: "oc_sk_"}, {Kind: "NS_KEY", Regex: "ns_[0-9a-f]{12}"}, {Kind: "LAST", Prefix: "zz_tok_"}}
	if rec.Code != http.StatusOK || !slices.Equal(settings.Load().RedactRules, want) || !strings.Contains(rec.Body.String(), `"kind":"NS_KEY","regex":"ns_[0-9a-f]{12}"`) {
		t.Fatalf("edit: %d %s, saved %v", rec.Code, rec.Body, settings.Load().RedactRules)
	}
	if !masks("key ns_00000000000b here") || masks("key ns-00000000000c here") {
		t.Fatal("the gateway's masking didn't follow the edit")
	}
	// a prefix rule becomes a pattern one, in its place
	if rec := post("/api/settings/redact-rule", `{"at":0,"was":{"kind":"GW_KEY","prefix":"oc_sk_"},"rule":{"kind":"GW_KEY","regex":"oc_sk_[A-Za-z0-9]{20,}"}}`); rec.Code != http.StatusOK || settings.Load().RedactRules[0] != (redact.Rule{Kind: "GW_KEY", Regex: "oc_sk_[A-Za-z0-9]{20,}"}) {
		t.Fatalf("prefix to regex: %d %s, saved %v", rec.Code, rec.Body, settings.Load().RedactRules)
	}
	want[0] = redact.Rule{Kind: "GW_KEY", Regex: "oc_sk_[A-Za-z0-9]{20,}"}

	for _, c := range []struct{ name, body, says string }{
		{"bad regex", `{"at":1,"was":{"kind":"NS_KEY","regex":"ns_[0-9a-f]{12}"},"rule":{"kind":"NS_KEY","regex":"ns_[0-9a-f"}}`, "doesn't work"},
		{"matches nothing", `{"at":1,"was":{"kind":"NS_KEY","regex":"ns_[0-9a-f]{12}"},"rule":{"kind":"NS_KEY","regex":"x*"}}`, "matches nothing"},
		{"short prefix", `{"at":2,"was":{"kind":"LAST","prefix":"zz_tok_"},"rule":{"kind":"LAST","prefix":"z"}}`, "prefix"},
		{"empty", `{"at":2,"was":{"kind":"LAST","prefix":"zz_tok_"},"rule":{"kind":"LAST","prefix":"  "}}`, "needs a prefix or a regular expression"},
		{"another rule", `{"at":2,"was":{"kind":"LAST","prefix":"zz_tok_"},"rule":{"kind":"ns key","regex":"ns_[0-9a-f]{12}"}}`, "there already"},
		{"out of range", `{"at":3,"was":{"kind":"LAST","prefix":"zz_tok_"},"rule":{"kind":"X","prefix":"xx_tok_"}}`, "no longer there"},
		{"negative", `{"at":-1,"was":{"kind":"LAST","prefix":"zz_tok_"},"rule":{"kind":"X","prefix":"xx_tok_"}}`, "no longer there"},
		{"changed meanwhile", `{"at":1,"was":{"kind":"NS","regex":"ns-[0-9a-f]{12}"},"rule":{"kind":"X","prefix":"xx_tok_"}}`, "no longer there"},
	} {
		rec := post("/api/settings/redact-rule", c.body)
		if rec.Code != http.StatusBadRequest || !strings.Contains(rec.Body.String(), c.says) {
			t.Errorf("%s: %d %s, want refused saying %q", c.name, rec.Code, rec.Body, c.says)
		}
		if got := settings.Load().RedactRules; !slices.Equal(got, want) {
			t.Errorf("%s: the rules changed to %v", c.name, got)
		}
	}
	if !masks("key ns_00000000000d here") {
		t.Fatal("a refused edit changed what is masked")
	}
}
