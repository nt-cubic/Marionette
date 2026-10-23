package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A request with a value of a kind the user set to block goes nowhere,
// on each protocol: 400, a message that names the kind and the value's
// hint but never the value, and a record of it held back (#1977).
func TestBlockedKindHoldsTheRequestBack(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	var hit atomic.Int32
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hit.Add(1)
		w.WriteHeader(500)
	}))
	defer up.Close()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1", "embed-1"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactActions: map[string]string{"API_KEY": "block"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, tc := range []struct{ path, body string }{
		{"/v1/chat/completions", `{"model":"fake/m1","messages":[{"role":"user","content":"key ` + key + `"}]}`},
		{"/v1/messages", `{"model":"fake/m1","max_tokens":10,"messages":[{"role":"user","content":"key ` + key + `"}]}`},
		{"/v1/responses", `{"model":"fake/m1","input":"key ` + key + `"}`},
		{"/v1/embeddings", `{"model":"fake/embed-1","input":"key ` + key + `"}`},
	} {
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body)))
		raw := rec.Body.String()
		if rec.Code != 400 || strings.Contains(raw, key[3:]) || !strings.Contains(raw, "API_KEY") || !strings.Contains(raw, "sk-") {
			t.Fatalf("%s: %d %s", tc.path, rec.Code, raw)
		}
		rs, _ := maskrec.List(0, 1, maskrec.Filter{})
		if len(rs) != 1 || !rs[0].Blocked || len(rs[0].Values) != 1 || rs[0].Values[0].Action != "block" {
			t.Fatalf("%s: recorded %+v", tc.path, rs)
		}
	}
	if n := hit.Load(); n != 0 {
		t.Fatalf("the vendor was sent %d requests", n)
	}
}

// A kind the user set to warn of goes to the vendor as written, and the
// record tells it, with no snip of it; the rest is masked.
func TestWarnedKindGoesAsWritten(t *testing.T) {
	const key, mail = "sk-proj-abcdEFGH1234ijklMNOP5678qrst", "ops.lead@kelvarn.io"
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	defer up.Close()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactActions: map[string]string{"EMAIL": "warn"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"fake/m1","messages":[{"role":"user","content":"mail `+mail+` key `+key+`"}]}`)))
	if rec.Code != 200 || !strings.Contains(string(got), mail) || strings.Contains(string(got), key) {
		t.Fatalf("%d, sent %s", rec.Code, got)
	}
	rs, _ := maskrec.List(0, 1, maskrec.Filter{})
	if len(rs) != 1 || rs[0].N != 1 || len(rs[0].Values) != 2 {
		t.Fatalf("recorded %+v", rs)
	}
	for _, v := range rs[0].Values {
		if v.Kind == "EMAIL" && (v.Action != "warn" || rs[0].Snips[v.Placeholder] != "") {
			t.Fatalf("warned value %+v, snip %q", v, rs[0].Snips[v.Placeholder])
		}
	}
	ts := s.Trace(t.Context(), 0, 0)
	for _, rt := range ts.Routes {
		if rt.ID == rs[0].Route && (rt.Masked == nil || rt.Masked.Warned != 1 || rt.Masked.Distinct != 1) {
			t.Fatalf("route's masked: %+v", rt.Masked)
		}
	}
}
