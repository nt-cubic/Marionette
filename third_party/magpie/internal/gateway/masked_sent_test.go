package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/maskrec"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A masked request's record has, in memory, the body as its vendor got
// it, how long masking took, and, once the answer is done, how many
// places of the answer had each value put back: the 脱敏 page reads them.
func TestMaskedRecordKeepsSentAndBack(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	ph := regexp.MustCompile(`\{\{API_KEY_[0-9a-z]{8}\}\}`)
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = io.ReadAll(r.Body)
		p := ph.FindString(string(got))
		b, _ := json.Marshal(p + " and " + p)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":`+string(b)+`},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
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
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"fake/m1","messages":[{"role":"user","content":"key `+key+`"}]}`)))
	p := ph.FindString(string(got))
	if rec.Code != 200 || p == "" || strings.Count(rec.Body.String(), key) != 2 {
		t.Fatalf("%d, sent %s, answered %s", rec.Code, got, rec.Body)
	}
	rs, _ := maskrec.List(0, 1, maskrec.Filter{})
	if len(rs) != 1 {
		t.Fatal("nothing recorded")
	}
	r, ok := maskrec.Get(rs[0].Key)
	if !ok || r.Back[p] != 2 || r.Took <= 0 {
		t.Fatalf("record: %+v", r)
	}
	sent, ok := maskrec.SentOf(r.Key)
	if !ok || !strings.Contains(sent, p) || strings.Contains(sent, key) {
		t.Fatalf("sent %q %v", sent, ok)
	}
}
