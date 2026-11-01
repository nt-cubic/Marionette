package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// openRouterUp stands in for openrouter.ai, in process: /api/v1/systemone
// answers Mercury Decide's way (the wrapper as OpenRouter served it on
// 2026-11-01), and anything else gets the 400 coeo91 saw. Requests for any
// other host go to the test's own servers.
type openRouterUp struct {
	mu    sync.Mutex
	asked []string // path and model of each request to openrouter.ai
	next  http.RoundTripper
}

func (u *openRouterUp) RoundTrip(r *http.Request) (*http.Response, error) {
	if r.URL.Hostname() != "openrouter.ai" {
		return u.next.RoundTrip(r)
	}
	b, _ := io.ReadAll(r.Body)
	var q struct{ Model string }
	json.Unmarshal(b, &q)
	u.mu.Lock()
	u.asked = append(u.asked, r.URL.Path+" "+q.Model)
	u.mu.Unlock()
	code, body := 400, `{"error":{"message":"`+q.Model+` is a decisions model and cannot be used with the chat/completions endpoint. Use the /api/alpha/decisions endpoint instead.","code":400}}`
	if r.URL.Path == "/api/v1/systemone" {
		code, body = 200, `{"model":"inception/mercury-decide-20260930","answers":{"intent":{"type":"choice","choice":"debugging","confidence":0.9}},"usage":{"input_tokens":72,"output_tokens":1,"cost":0.00000144},"id":"gen-dec-1793467694-KCls8tvRNbusIJmB9FhD","provider":"Inception"}`
	}
	return &http.Response{StatusCode: code, Header: http.Header{"Content-Type": {"application/json"}},
		Body: io.NopCloser(strings.NewReader(body)), Request: r}, nil
}

func (u *openRouterUp) got() []string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return slices.Clone(u.asked)
}

// coeo91 on Discord: OpenRouter's inception/mercury-decide:free, picked by
// hand into a group used as smart routing's decision model, was sent to
// chat/completions and refused 400 ("is a decisions model"). The variant of
// a model OpenRouter lists as a decision model is one too: the group of it
// is asked at /api/v1/systemone, and a chat or count_tokens request naming
// it is refused here, before OpenRouter is asked.
func TestOpenRouterDecisionVariantNeverChats(t *testing.T) {
	_, _, _ = ruled(t)
	const variant = "inception/mercury-decide:free"
	const base = "https://openrouter.ai/api/v1"
	if err := provider.Save(provider.Provider{ID: "openrouter", Name: "OpenRouter", Preset: "openrouter", Key: "kor",
		Chat: base, Models: []string{"openai/gpt-5", variant}}); err != nil {
		t.Fatal(err)
	}
	// OpenRouter's two lists: the decisions list names the model once,
	// without :free
	if err := catalog.SaveLive("openrouter", base, []catalog.Model{{ID: "openai/gpt-5", Context: 400000}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("openrouter.decisions", base, []catalog.Model{{ID: "inception/mercury-decide", Context: 65536}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Decide", Members: []string{"openrouter/" + variant}}); err != nil {
		t.Fatal(err)
	}
	g, _, _ := provider.FindGroup("group/r")
	g.Rules, g.Classifier = []provider.Rule{{Use: "b/big", Intent: "debugging"}}, "group/decide"
	if err := provider.SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	up := &openRouterUp{next: http.DefaultTransport}
	old := http.DefaultClient.Transport
	http.DefaultClient.Transport = up
	t.Cleanup(func() { http.DefaultClient.Transport = old })
	s := New()
	s.client = &http.Client{Transport: up}

	// the classifier asks the group at /systemone, and the turn goes where
	// its answer says
	out, r := postOK(t, s, "or1", chat("why does this crash?", nil, 0, ""))
	if !strings.Contains(out, "from kb") || r.Rule.Classified.Intent != "debugging" || r.Rule.Classified.Error != "" {
		t.Fatalf("classified: %s %+v", out, r.Rule.Classified)
	}
	if got := up.got(); !slices.Equal(got, []string{"/api/v1/systemone " + variant}) {
		t.Fatalf("OpenRouter asked: %v", got)
	}

	// an agent's chat naming it is refused here
	code, out := postAs(t, s, "or2", `{"model":"openrouter/`+variant+`","messages":[{"role":"user","content":"hi"}]}`)
	if code != 400 || !strings.Contains(out, "only decides") {
		t.Fatalf("chat: %d %s", code, out)
	}
	// and so is counting its tokens, for the model and for its group
	for _, m := range []string{"openrouter/" + variant, "group/decide"} {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", "/v1/messages/count_tokens", strings.NewReader(
			`{"model":"`+m+`","messages":[{"role":"user","content":"hi"}]}`))
		s.Handler().ServeHTTP(rec, req)
		if rec.Code != 400 || !strings.Contains(rec.Body.String(), "only decides") {
			t.Fatalf("count_tokens %s: %d %s", m, rec.Code, rec.Body.String())
		}
	}
	if got := up.got(); len(got) != 1 {
		t.Fatalf("OpenRouter asked: %v", got)
	}
}
