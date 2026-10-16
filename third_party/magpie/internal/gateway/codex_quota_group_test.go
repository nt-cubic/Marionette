package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The ChatGPT backend saying a Codex account is out of its allowance, in
// each shape it says it: a 429 body, and a stream that fails with it.
var codexSpent = []struct {
	name   string
	refuse func(w http.ResponseWriter)
}{
	{"429 json", func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(429)
		io.WriteString(w, `{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":7200}}`)
	}},
	{"429 bare", func(w http.ResponseWriter) {
		w.WriteHeader(429)
		io.WriteString(w, `{"error":{"type":"usage_limit_reached","message":"You've hit your usage limit. Upgrade to Pro, or try again at 9:34 PM.","plan_type":"plus","resets_at":4102444800}}`)
	}},
	{"response.failed usage_limit_reached", func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`event: response.created`+"\n"+`data: {"type":"response.created","sequence_number":0,"response":{"id":"r0","status":"in_progress"}}`,
			`event: codex.rate_limits`+"\n"+`data: {"type":"codex.rate_limits","plan_type":"plus","rate_limits":{"primary":{"used_percent":100.0,"window_minutes":300,"reset_after_seconds":7200}}}`,
			`event: response.failed`+"\n"+`data: {"type":"response.failed","sequence_number":1,"response":{"id":"r0","status":"failed","error":{"code":"usage_limit_reached","message":"You've hit your usage limit. Upgrade to Pro, or try again in 2 hours."}}}`))
	}},
	{"response.failed rate_limit_exceeded", func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r0","status":"in_progress"}}`,
			`event: response.failed`+"\n"+`data: {"type":"response.failed","response":{"id":"r0","status":"failed","error":{"code":"rate_limit_exceeded","message":"Rate limit reached for gpt-5.5 on tokens per min. Please try again in 11.054s."}}}`))
	}},
	{"stream without content type", func(w http.ResponseWriter) {
		// the ChatGPT backend streams without saying so
		io.WriteString(w, sse(`data: {"type":"response.created","response":{"id":"r0","status":"in_progress"}}`,
			`data: {"type":"response.failed","response":{"id":"r0","status":"failed","error":{"code":"usage_limit_reached","message":"You've hit your usage limit."}}}`))
	}},
	{"error event", func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r0"}}`,
			`event: error`+"\n"+`data: {"type":"error","error":{"type":"usage_limit_reached","code":"usage_limit_reached","message":"The usage limit has been reached","resets_in_seconds":7200}}`))
	}},
	{"usage_not_included", func(w http.ResponseWriter) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(403)
		io.WriteString(w, `{"error":{"type":"usage_not_included","message":"To use Codex with your ChatGPT plan, upgrade to Plus."}}`)
	}},
}

var grokAnswer = sse(`data: {"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"}}]}`,
	`data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
	`data: [DONE]`)

// A routing group with a Codex subscription first and a Grok-like provider
// after it (Discord, waroy): the Codex account out of its allowance, in any
// shape the ChatGPT backend says it, hands the turn to the next member, and
// Codex is answered by it rather than handed the refusal.
func TestCodexSpentMovesToNextGroupMember(t *testing.T) {
	for _, x := range codexSpent {
		for _, path := range []string{CodexPath + "/responses", "/v1/responses"} {
			t.Run(x.name+" "+path, func(t *testing.T) {
				codexSignedIn(t)
				sticks.Lock()
				sticks.m = map[string]stick{}
				sticks.Unlock()
				var tried []string
				chatgptRefusing(t, &tried, x.refuse)
				grok := &scripted{replies: []reply{{200, "text/event-stream", grokAnswer}}}
				scriptedOn(t, "xai", provider.Chat, grok)
				refusalGroup(t, "codex/gpt-5.5", "xai/m")
				s := New()
				rec := codexOn(s, path, codexAsk)
				if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello") || len(tried) != 1 || grok.n != 1 {
					t.Fatalf("status %d: %s (codex %v, grok %d)", rec.Code, rec.Body.String(), tried, grok.n)
				}
				r := lastRoute(s)
				if len(r.Tries) != 2 || r.Tries[0].Status < 400 || r.Tries[1].Status != 200 {
					t.Fatalf("tries: %+v", r.Tries)
				}

				// the next turn goes to grok while the account is out
				tried = nil
				rec = codexOn(s, path, codexAsk)
				if rec.Code != 200 || grok.n != 2 {
					t.Fatalf("second turn %d: %s (codex %v, grok %d)", rec.Code, rec.Body.String(), tried, grok.n)
				}
			})
		}
	}
}

// codexOn is Codex, signed in to ChatGPT, asking s at path.
func codexOn(s *Server, path, body string) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", path, strings.NewReader(body))
	req.Header.Set("Authorization", "Bearer chatgpt-token")
	req.Header.Set("chatgpt-account-id", "acct-1")
	s.Handler().ServeHTTP(rec, req)
	return rec
}

// The same with the conversation begun on the Codex account: its reasoning,
// sealed by OpenAI, goes to a Grok that can't read it, which is asked again
// without it.
func TestCodexSpentMovesToGrokWithSealedReasoning(t *testing.T) {
	for _, x := range codexSpent {
		for _, inStream := range []bool{false, true} {
			t.Run(x.name, func(t *testing.T) {
				codexSignedIn(t)
				sticks.Lock()
				sticks.m = map[string]stick{}
				sticks.Unlock()
				refusedSeals.Lock()
				refusedSeals.m = map[string]sealsRefused{}
				refusedSeals.Unlock()
				var tried []string
				chatgptRefusing(t, &tried, x.refuse)
				g := &sealer{name: "grok", stream: inStream}
				gup := httptest.NewServer(g)
				t.Cleanup(gup.Close)
				if err := provider.Save(provider.Provider{ID: "grok1", Name: "Grok (SuperGrok)", Key: "k", Models: []string{"grok-4"}, Responses: gup.URL + "/v1"}); err != nil {
					t.Fatal(err)
				}
				refusalGroup(t, "codex/gpt-5.5", "grok1/grok-4")
				s := New()
				in := `{"type":"message","role":"user","content":[{"type":"input_text","text":"count the lines"}]},` +
					`{"type":"reasoning","id":"rs_o","summary":[],"content":null,"encrypted_content":"sealed-by-openai"},` +
					`{"type":"function_call","call_id":"c1","name":"shell","arguments":"{}"},{"type":"function_call_output","call_id":"c1","output":"ok"}`
				rec := httptest.NewRecorder()
				req := httptest.NewRequest("POST", CodexPath+"/responses", strings.NewReader(`{"model":"group/g","stream":true,"input":[`+in+`]}`))
				req.Header.Set("Authorization", "Bearer chatgpt-token")
				req.Header.Set("chatgpt-account-id", "acct-1")
				req.Header.Set("session_id", "thread-1")
				s.Handler().ServeHTTP(rec, req)
				if rec.Code != 200 || !strings.Contains(rec.Body.String(), "from grok") {
					t.Fatalf("stream=%v %d: %s (codex %v, grok %q)", inStream, rec.Code, rec.Body.String(), tried, g.sent())
				}
			})
		}
	}
}

// usedUpShapes runs for the shapes of codexSpent saying the allowance is
// gone, rather than a moment's rate limit or a plan without Codex.
func usedUpShapes(t *testing.T, run func(t *testing.T, refuse func(w http.ResponseWriter))) {
	for _, x := range codexSpent {
		if x.name == "response.failed rate_limit_exceeded" || x.name == "usage_not_included" {
			continue
		}
		t.Run(x.name, func(t *testing.T) { run(t, x.refuse) })
	}
}

// The turn after the Codex account ran out (Discord, waroy), Grok is busy
// for a moment: Grok is asked again, as the last that may answer, rather
// than the turn going to the spent account, whose usage-limit error makes
// Codex stop taking input.
func TestCodexSpentGrokBusyIsAskedAgain(t *testing.T) {
	usedUpShapes(t, func(t *testing.T, refuse func(w http.ResponseWriter)) {
		codexSignedIn(t)
		sticks.Lock()
		sticks.m = map[string]stick{}
		sticks.Unlock()
		var tried []string
		chatgptRefusing(t, &tried, refuse)
		grok := &scripted{replies: []reply{{200, "text/event-stream", grokAnswer}, {503, "", `{"error":{"message":"upstream busy"}}`}, {200, "text/event-stream", grokAnswer}}}
		scriptedOn(t, "xai", provider.Chat, grok)
		refusalGroup(t, "codex/gpt-5.5", "xai/m")
		s := New()
		for turn := range 2 {
			rec := codexOn(s, CodexPath+"/responses", codexAsk)
			if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello") {
				t.Fatalf("turn %d %d: %s (codex %v, grok %d)", turn, rec.Code, rec.Body.String(), tried, grok.n)
			}
		}
		if len(tried) != 1 || grok.n != 3 {
			t.Fatalf("codex %v, grok %d", tried, grok.n)
		}
	})
}

// And when Grok keeps failing, Codex is told Grok's error, not that its
// usage is exhausted.
func TestCodexSpentGrokFailingIsTold(t *testing.T) {
	usedUpShapes(t, func(t *testing.T, refuse func(w http.ResponseWriter)) {
		codexSignedIn(t)
		sticks.Lock()
		sticks.m = map[string]stick{}
		sticks.Unlock()
		var tried []string
		chatgptRefusing(t, &tried, refuse)
		grok := &scripted{replies: []reply{{200, "text/event-stream", grokAnswer}, {503, "", `{"error":{"message":"upstream busy"}}`}}}
		scriptedOn(t, "xai", provider.Chat, grok)
		refusalGroup(t, "codex/gpt-5.5", "xai/m")
		s := New()
		if rec := codexOn(s, CodexPath+"/responses", codexAsk); rec.Code != 200 {
			t.Fatalf("first turn %d: %s", rec.Code, rec.Body.String())
		}
		rec := codexOn(s, CodexPath+"/responses", codexAsk)
		body := rec.Body.String()
		if rec.Code != 503 || !strings.Contains(body, "upstream busy") || strings.Contains(strings.ToLower(body), "usage limit") {
			t.Fatalf("%d: %s (codex %v, grok %d)", rec.Code, body, tried, grok.n)
		}
		// the Routing page tells the agent got another's error, not this one's (#2281)
		if r := lastRoute(s); r.Tries[len(r.Tries)-1].ID != "codex" || r.Tries[len(r.Tries)-1].Fail != failQuota || !r.Tries[len(r.Tries)-1].Earlier {
			t.Fatalf("tries: %+v", r.Tries)
		}
	})
}
