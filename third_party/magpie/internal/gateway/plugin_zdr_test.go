package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// Command Code's own answers to x-cmd-zdr: 1 for a model no Zero Data
// Retention upstream serves, as they came on 2026-10-19 for
// poolside/laguna-s-2.1-free (magpie-community/plugins#246, its
// zdr.test.mjs): the Provider API's 422, and the 400 the plugin makes of
// Go's /alpha/generate error line.
const (
	zdrProvider422 = `{"error":{"message":"This model has no zero-data-retention upstream. Remove the x-cmd-zdr header or choose a different model.","type":"invalid_request_error","code":"cmd_zdr_no_providers"}}`
	zdrGo400       = `{"error":{"message":"This model has no zero-data-retention upstream. Disable CMD_ZDR or pick a different model.","type":"cmd_zdr_no_providers","code":400}}`
)

// noneRested says nothing of the provider id rests or has a failure
// counted against it.
func noneRested(t *testing.T, id string) {
	t.Helper()
	restingUntil.Lock()
	for k, until := range restingUntil.m {
		if strings.HasPrefix(k, id) && time.Now().Before(until) {
			t.Errorf("%s rests until %s", k, until)
		}
	}
	restingUntil.Unlock()
	routed.Lock()
	for k, n := range routed.failures {
		if strings.HasPrefix(k, id) && n > 0 {
			t.Errorf("%s has %d failures counted", k, n)
		}
	}
	routed.Unlock()
}

// A Zero Data Retention refusal from Command Code (plugin 0.1.14's switch)
// is about that request's setting, not the model or the account: nothing
// rests, so the model answers again as soon as the switch is off; in a
// group the next member, at another vendor, is asked; the account's mates
// at the same model, sent the same header, aren't; and alone, the agent
// has Command Code's own words. Before, the 422's "…a different
// model.","type":"invalid_request_error" read as the model unserved
// (unservedWords) and rested it on the account.
func TestCommandCodeZDRRefusalRestsNothing(t *testing.T) {
	for _, x := range []struct {
		name, body string
		status     int
	}{
		{"provider api", zdrProvider422, 422},
		{"go", zdrGo400, 400},
	} {
		t.Run(x.name, func(t *testing.T) {
			var mu sync.Mutex
			zdr := true
			var asked []string
			movedFake(t, "commandcode-plan", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				io.ReadAll(r.Body)
				mu.Lock()
				on := zdr
				asked = append(asked, r.Header.Get("Authorization"))
				mu.Unlock()
				w.Header().Set("Content-Type", "application/json")
				if on {
					w.WriteHeader(x.status)
					io.WriteString(w, x.body)
					return
				}
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"from commandcode"},"finish_reason":null}]}`+"\n\n")
				io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
			}))
			// a second Command Code account: the switch is the provider's,
			// so it is sent the same header
			ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
			defer cancel()
			if _, err := plugin.APIKey(ctx, "commandcode-plan", 0, nil, "k2", plugin.NewAccount); err != nil {
				t.Fatal(err)
			}
			if _, err := plugin.Providers(ctx); err != nil {
				t.Fatal(err)
			}
			if p, err := provider.Find("commandcode-plan"); err != nil || len(p.AlsoOn()) != 1 {
				t.Fatalf("Find = %+v, %v", p, err)
			}
			b := &scripted{replies: []reply{{200, "", chatOK}, {200, "text/event-stream", chatStreamOK}}}
			scriptedOn(t, "b", provider.Chat, b)
			if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"commandcode-plan/fake-1", "b/m"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			s := New()
			reset := func() {
				mu.Lock()
				asked = nil
				mu.Unlock()
			}
			n := func() int {
				mu.Lock()
				defer mu.Unlock()
				return len(asked)
			}

			// alone: one account asked, the agent has Command Code's words
			code, body := postAs(t, s, "", `{"model":"commandcode-plan/fake-1","messages":[{"role":"user","content":"hi"}]}`)
			if code != x.status || !strings.Contains(body, "no zero-data-retention upstream") || n() != 1 {
				t.Fatalf("alone: %d %s (asked %d)", code, body, n())
			}
			noneRested(t, "commandcode-plan")

			// in a group: the next member answers, and nothing rests
			reset()
			code, body = postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
			if code != 200 || !strings.Contains(body, "hello") || n() != 1 || b.n != 1 {
				t.Fatalf("group: %d %s (commandcode %d, b %d)", code, body, n(), b.n)
			}
			r := s.trace.routes[len(s.trace.routes)-1]
			if len(r.Tries) != 2 || r.Tries[0].Status != x.status || r.Tries[0].Rest != nil {
				t.Fatalf("tries %+v", r.Tries)
			}
			noneRested(t, "commandcode-plan")

			// and for an agent speaking Anthropic's Messages, whose error
			// shape keeps no code
			reset()
			rec := httptest.NewRecorder()
			s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages",
				strings.NewReader(`{"model":"group/g","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`)))
			if rec.Code != 200 || !strings.Contains(rec.Body.String(), "from b") || n() != 1 || b.n != 2 {
				t.Fatalf("messages: %d %s (commandcode %d, b %d)", rec.Code, rec.Body, n(), b.n)
			}
			rec = httptest.NewRecorder()
			s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages",
				strings.NewReader(`{"model":"commandcode-plan/fake-1","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`)))
			if rec.Code != x.status || !strings.Contains(rec.Body.String(), "no zero-data-retention upstream") || n() != 2 {
				t.Fatalf("messages alone: %d %s (commandcode %d)", rec.Code, rec.Body, n())
			}
			noneRested(t, "commandcode-plan")

			// the switch off: Command Code is asked first again, and answers
			mu.Lock()
			zdr = false
			mu.Unlock()
			reset()
			code, body = postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
			if code != 200 || !strings.Contains(body, "from commandcode") || n() != 1 || b.n != 2 {
				t.Fatalf("switch off: %d %s (commandcode %d, b %d)", code, body, n(), b.n)
			}
		})
	}
}

// Only Command Code's own error code makes a refusal a setting's: another
// vendor's model refusal in the same words, or one in the same OpenAI
// error shape, still rests the model as before.
func TestZDRRefusalIsCommandCodesCodeOnly(t *testing.T) {
	for _, x := range []struct {
		status    int
		body      string
		zdr, rest bool // zdrRefused, modelRefused
	}{
		{422, zdrProvider422, true, false},
		{400, zdrGo400, true, false},
		{400, `Command Code: ` + zdrGo400, true, false},
		// the same words with no such code are another vendor's
		{422, `{"error":{"message":"This model has no zero-data-retention upstream. Remove the x-cmd-zdr header or choose a different model.","type":"invalid_request_error"}}`, false, true},
		{404, `{"error":{"message":"The model 'gpt-9' does not exist","type":"invalid_request_error","code":"model_not_found"}}`, false, true},
		{400, `{"error":{"message":"model \"gpt-6-sol\" is not accessible via the /chat/completions endpoint","code":"unsupported_api_for_model"}}`, false, true},
		// the code named in a message, not as the error's type or code
		{400, `{"error":{"message":"unknown model; see cmd_zdr_no_providers","type":"invalid_request_error"}}`, false, true},
		// not a refusal
		{200, zdrProvider422, false, false},
	} {
		if got := zdrRefused(x.status, []byte(x.body)); got != x.zdr {
			t.Errorf("zdrRefused(%d, %s) = %v", x.status, x.body, got)
		}
		if got := modelRefused(x.status, []byte(x.body)); got != x.rest {
			t.Errorf("modelRefused(%d, %s) = %v", x.status, x.body, got)
		}
	}
}

// Wherever a try's failure is rested — a strict conversation's own
// account, a System One decision, a count — a Zero Data Retention refusal
// rests nothing: read from the vendor's own body, or from magpie's note
// beside the error the agent was given in Anthropic's shape, which keeps
// no code. Another vendor's model refusal in the same words still rests
// the model.
func TestZDRRefusalNeverRests(t *testing.T) {
	fresh(t)
	s := New()
	c := candidate{p: provider.Provider{ID: "commandcode-plan", Name: "Command Code Plan", Key: "k"}, model: "m", rest: "commandcode-plan"}
	noted := http.Header{}
	keepRetry(noted, http.Header{}, []byte(zdrGo400))
	anthropicShaped := []byte(`{"error":{"message":"Command Code Plan: This model has no zero-data-retention upstream. Disable CMD_ZDR or pick a different model.","type":"invalid_request_error"},"type":"error"}`)
	for _, x := range []struct {
		status int
		header http.Header
		body   []byte
	}{
		{422, http.Header{}, []byte(zdrProvider422)},
		{400, http.Header{}, []byte(zdrGo400)},
		{400, noted, anthropicShaped},
	} {
		s.restAfter(c, x.status, x.header, x.body)
		if r, ok := restingOf(c); ok {
			t.Fatalf("%d %s rests: %+v", x.status, x.body, r)
		}
	}
	noneRested(t, "commandcode-plan")
	// without the note, the same words rest the model, as any vendor's do
	s.restAfter(c, 400, http.Header{}, anthropicShaped)
	if _, ok := restingOf(c); !ok {
		t.Fatal("a model refusal without Command Code's code rests nothing")
	}
}
