package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// reasoningLeft says what of a sent body still asks the model to reason.
func reasoningLeft(sent map[string]any) string {
	for _, k := range []string{"reasoning_effort", "reasoning", "thinking", "enable_thinking", "output_config"} {
		if sent[k] != nil {
			return k
		}
	}
	return ""
}

// A model the user ticked no levels for is sent no reasoning field at all,
// whatever the agent asks: yuexian7's vendor lists the model as reasoning
// on models.dev and answers 422 to any reasoning parameter. It holds on
// every route — a Chat and a Responses request relayed as they are, an
// Anthropic request translated to Chat — asked of the model by name and
// through a group, and a group with it as a member keeps the other
// member's levels and still sends that member the effort asked.
func TestReasonlessModelSentNoReasoning(t *testing.T) {
	fresh(t)
	thinks, quiet, reset := quietGroup(t)
	if err := provider.SetModelReasonless("thinks/levelled", true); err != nil {
		t.Fatal(err)
	}
	s := New()
	post := func(path, body string) string {
		t.Helper()
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", path, strings.NewReader(body))
		req.Header.Set("x-session-id", "r1")
		s.Handler().ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("%s %s: %d %s", path, body, rec.Code, rec.Body)
		}
		return rec.Body.String()
	}
	for _, model := range []string{"thinks/levelled", "group/mix"} {
		for _, c := range []struct{ path, body string }{
			{"/v1/chat/completions", `{"model":"` + model + `","reasoning_effort":"high","messages":[{"role":"user","content":"hi"}]}`},
			{"/v1/chat/completions", `{"model":"` + model + `","reasoning_effort":"high","enable_thinking":true,"messages":[{"role":"user","content":"hi"}]}`},
			{"/v1/responses", `{"model":"` + model + `","input":"hi","reasoning":{"effort":"high","summary":"auto"},"include":["reasoning.encrypted_content"]}`},
			{"/v1/messages", `{"model":"` + model + `","max_tokens":2000,"thinking":{"type":"enabled","budget_tokens":1024},"output_config":{"effort":"high"},"messages":[{"role":"user","content":"hi"}]}`},
		} {
			reset(0)
			if out := post(c.path, c.body); !strings.Contains(out, "from kt") {
				t.Fatalf("%s %s: %s", model, c.path, out)
			}
			sent := thinks.sent()
			if k := reasoningLeft(sent); k != "" {
				t.Fatalf("%s %s: the reasonless model was sent %s: %v", model, c.path, k, sent)
			}
			if inc, _ := json.Marshal(sent["include"]); strings.Contains(string(inc), "reasoning") {
				t.Fatalf("%s %s: sent include %s", model, c.path, inc)
			}
		}
	}
	// a group with a reasonless member keeps the levels its other members
	// take, and a member that reasons is still sent the effort asked
	if err := provider.SaveGroup(provider.Group{Name: "Two", Members: []string{"thinks/levelled", "quiet/plain:high"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	reset(500)
	if out := post("/v1/chat/completions", `{"model":"group/two","reasoning_effort":"high","messages":[{"role":"user","content":"hi"}]}`); !strings.Contains(out, "from kq") {
		t.Fatalf("two: %s", out)
	}
	if sent := quiet.sent(); sent["reasoning_effort"] != "high" {
		t.Fatalf("the member that reasons was sent %v", sent)
	}
	// ticking a level again sends reasoning as before
	if err := provider.SetModelEfforts("thinks/levelled", []string{"low", "high"}); err != nil {
		t.Fatal(err)
	}
	reset(0)
	post("/v1/chat/completions", `{"model":"thinks/levelled","reasoning_effort":"high","messages":[{"role":"user","content":"hi"}]}`)
	if sent := thinks.sent(); sent["reasoning_effort"] != "high" {
		t.Fatalf("ticked again, it was sent %v", sent)
	}
}

// anthUp is an Anthropic vendor that keeps what it was sent.
type anthUp struct {
	mu   sync.Mutex
	last string
}

func (u *anthUp) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	u.mu.Lock()
	u.last = string(b)
	u.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	io.WriteString(w, `{"id":"m1","type":"message","role":"assistant","model":"glm","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`)
}

// Relayed to an Anthropic vendor as it came, a reasonless model is sent
// neither the agent's thinking nor the thinking:{type:disabled} magpie
// adds for a model that isn't Claude: that is a reasoning field too.
func TestReasonlessAnthropicRelay(t *testing.T) {
	fresh(t)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	up := &anthUp{}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "anth", Name: "Anth", Key: "k", Models: []string{"glm"}, Anthropic: srv.URL}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModelReasonless("anth/glm", true); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, body := range []string{
		`{"model":"anth/glm","max_tokens":2000,"thinking":{"type":"enabled","budget_tokens":1024},"output_config":{"effort":"high"},"messages":[{"role":"user","content":"hi"}]}`,
		`{"model":"anth/glm","max_tokens":2000,"messages":[{"role":"user","content":"hi"}]}`,
	} {
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body)
		}
		up.mu.Lock()
		var sent map[string]any
		json.Unmarshal([]byte(up.last), &sent)
		up.mu.Unlock()
		if k := reasoningLeft(sent); k != "" {
			t.Fatalf("%s: sent %s: %v", body, k, sent)
		}
	}
}
