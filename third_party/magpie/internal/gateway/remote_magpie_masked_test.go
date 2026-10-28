package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A request another magpie passed on is named in this magpie's masking log
// by the agent that made it there, as its usage names it, not as "magpie"
// (linkaww on Discord: Pi's request through a local magpie was logged on
// the remote as magpie's). A client that isn't a magpie can't name another
// agent with magpie's header: it is logged as itself.
func TestRemoteMagpieMaskedNamesTheAgent(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	defer up.Close()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	s := New()
	logged := func(h map[string]string) maskrec.Record {
		t.Helper()
		before, _ := maskrec.List(0, 1, maskrec.Filter{})
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"fake/m1","messages":[{"role":"user","content":"key `+key+`"}]}`))
		for k, v := range h {
			req.Header.Set(k, v)
		}
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body.String())
		}
		rs, _ := maskrec.List(0, 1, maskrec.Filter{})
		if len(rs) != 1 || len(before) == 1 && rs[0].Key == before[0].Key {
			t.Fatalf("nothing recorded: %+v", rs)
		}
		return rs[0]
	}
	// what a magpie sends on for Pi (passOnCaller)
	if r := logged(map[string]string{"User-Agent": "magpie/0.1.0", AgentHeader: "pi", ViaHeader: "laptop"}); r.Agent != "pi" {
		t.Errorf("another magpie's request for Pi is logged as %q, want pi", r.Agent)
	}
	if r := logged(map[string]string{"User-Agent": "curl/8.0", AgentHeader: "pi"}); r.Agent != "curl" {
		t.Errorf("a client's own %s was taken: logged as %q, want curl", AgentHeader, r.Agent)
	}
}

// The requests in flight (/v1/magpie/concurrency) name another magpie's
// request by its agent too: they are counted before withCaller runs.
func TestRemoteMagpieActiveNamesTheAgent(t *testing.T) {
	s := New()
	req := httptest.NewRequest("POST", "/v1/chat/completions", nil)
	req.Header.Set("User-Agent", "magpie/0.1.0")
	req.Header.Set(AgentHeader, "pi")
	_, done := s.active.add(req)
	defer done()
	if as := s.Active(); len(as) != 1 || as[0].Agent != "pi" {
		t.Errorf("in flight: %+v, want pi's", as)
	}
}
