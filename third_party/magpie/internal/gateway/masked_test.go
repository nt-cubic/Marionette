package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A masked request is recorded for Settings → Privacy: each value by its
// placeholder, kind and the finder that found it, the text around it as
// the vendor saw it, and the route and call say how much was masked. The
// record never holds a value.
func TestMaskedRequestIsRecorded(t *testing.T) {
	const key, word = "sk-proj-abcdEFGH1234ijklMNOP5678qrst", "Bluefin"
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
	if err := settings.Save(settings.Settings{Redact: true, RedactWords: []string{word}}); err != nil {
		t.Fatal(err)
	}
	before, _ := maskrec.List(0, 1, maskrec.Filter{})
	s := New()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"fake/m1","messages":[{"role":"user","content":"project `+word+`, key `+key+`"},{"role":"user","content":"again `+key+`"}]}`))
	req.Header.Set("User-Agent", "claude-cli/2.1.0")
	s.Handler().ServeHTTP(rec, req)
	if rec.Code != 200 || strings.Contains(string(got), key) {
		t.Fatalf("%d, sent %s", rec.Code, got)
	}
	rs, _ := maskrec.List(0, 1, maskrec.Filter{})
	if len(rs) != 1 || len(before) == 1 && rs[0].Key == before[0].Key {
		t.Fatalf("nothing recorded: %+v", rs)
	}
	r := rs[0]
	if r.What != "chat" || r.Provider != "fake" || r.Route == 0 || r.N != 3 || len(r.Values) != 2 {
		t.Fatalf("record: %+v", r)
	}
	by := map[string]maskrec.Value{}
	for _, v := range r.Values {
		by[v.By] = v
		if strings.Contains(v.Placeholder+v.Head, word) || strings.Contains(v.Placeholder+v.Head, key[3:]) {
			t.Fatalf("value in the record: %+v", v)
		}
		if snip := r.Snips[v.Placeholder]; !strings.Contains(snip, v.Placeholder) || strings.Contains(snip, key) || strings.Contains(snip, word) {
			t.Fatalf("snip of %s: %q", v.Placeholder, snip)
		}
	}
	if by["rule"].Kind != "API_KEY" || by["rule"].N != 2 || by["rule"].Head != "sk-" || by["word"].Kind != "TERM" || by["word"].Len != len(word) {
		t.Fatalf("values: %+v", r.Values)
	}
	ts := s.Trace(context.Background(), 0, 0)
	found := false
	for _, rt := range ts.Routes {
		if rt.ID == r.Route {
			found = true
			if rt.Masked == nil || rt.Masked.N != 3 || rt.Masked.Distinct != 2 || rt.Masked.By["word"] != 1 || rt.Masked.Key != r.Key {
				t.Fatalf("route's masked: %+v", rt.Masked)
			}
		}
	}
	if !found {
		t.Fatalf("route %d not in the trace", r.Route)
	}
	// the GUI opens the record by this key: a JS number would round it
	if b, _ := json.Marshal(Masked{Key: r.Key}); !strings.Contains(string(b), `"key":"`+strconv.FormatInt(r.Key, 10)+`"`) {
		t.Fatalf("masked's key not a string: %s", b)
	}
	if c, ok := s.RouteCall(r.Route); !ok || c.Masked == nil || c.Masked.Distinct != 2 {
		t.Fatalf("call's masked: %+v %v", c.Masked, ok)
	}

	// a request with nothing to mask records nothing
	rec = httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"fake/m1","messages":[{"role":"user","content":"hello there"}]}`)))
	if again, _ := maskrec.List(0, 1, maskrec.Filter{}); rec.Code != 200 || len(again) != 1 || again[0].Key != r.Key {
		t.Fatalf("%d, recorded %+v", rec.Code, again)
	}
}

// A snip of a JSON body is the text of the string the value is in, its
// escapes undone, never the body's keys around it; plain text is cut
// by length alone.
func TestSnipStaysInItsString(t *testing.T) {
	const p = "{{API_KEY_abcd1234}}"
	hits := []gwmw.Hit{{Placeholder: p}}
	sent := `{"model":"m1","messages":[{"role":"user","content":"say \"hi\" to ` + p + ` now\nplease"}]}`
	if got, want := snipsOf(sent, hits)[p], `say "hi" to `+p+` now please`; got != want {
		t.Fatalf("JSON snip = %q, want %q", got, want)
	}
	long := strings.Repeat("a", 80) + ` "` + p + `" ` + strings.Repeat("b", 80)
	got := snipsOf(long, hits)[p]
	if !strings.HasPrefix(got, "…") || !strings.HasSuffix(got, "…") || !strings.Contains(got, `"`+p+`"`) {
		t.Fatalf("text snip = %q", got)
	}
}

// Two values a few words apart share one snip, even when the text runs
// past the window on both sides: the record shows the line once.
func TestSnipSharedByNearValues(t *testing.T) {
	const p, q = "{{PERSON_aaaa1111}}", "{{PERSON_bbbb2222}}"
	sent := strings.Repeat("x ", 40) + "Ask " + p + " whether the results for " + q + " came back " + strings.Repeat("y ", 40)
	got := snipsOf(sent, []gwmw.Hit{{Placeholder: p}, {Placeholder: q}})
	if got[p] == "" || got[p] != got[q] || !strings.Contains(got[p], q) {
		t.Fatalf("snips = %q", got)
	}
}
