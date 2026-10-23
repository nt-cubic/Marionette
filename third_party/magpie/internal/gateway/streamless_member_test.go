package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A member that answers a stream request with a whole JSON reply — a relay
// that doesn't speak SSE — fails the same way every time it is asked: the
// request is never read as a stream, however often it goes out. It is a
// failure of how the member answers, not of the moment it is asked in, so
// the group's patience doesn't keep asking it, and the one left isn't tried
// again on the chance a moment mends it. The next member of a group is
// asked, as its API may stream (#1418's patience is for what passes).
func TestStreamlessMemberIsAskedOnce(t *testing.T) {
	for _, c := range []struct {
		name  string
		query string
	}{
		{"whole", ``},
		{"stream", `"stream":true,`},
	} {
		t.Run(c.name, func(t *testing.T) {
			fresh(t)
			a := &scripted{replies: []reply{{200, "application/json", chatOK}}}
			scriptedOn(t, "a", provider.Chat, a)
			if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			s := New()
			rec := httptest.NewRecorder()
			req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(`{"model":"group/g","max_tokens":50,`+c.query+`"messages":[{"role":"user","content":"hi"}]}`))
			s.Handler().ServeHTTP(rec, req)
			if rec.Code != 502 || !strings.Contains(rec.Body.String(), "did not stream") {
				t.Fatalf("%d %s", rec.Code, rec.Body.String())
			}
			if a.n != 1 {
				t.Errorf("a member that never streams was asked %d times", a.n)
			}
		})
	}
}

// The group still moves on from it: the next member, whose API streams, is
// asked and answers.
func TestStreamlessMemberHandsOnToTheNextMember(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{200, "application/json", chatOK}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", sse(
		`data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}`,
		`data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}`,
		`data: {"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
	)}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(`{"model":"group/g","max_tokens":50,"stream":true,"messages":[{"role":"user","content":"hi"}]}`))
	s.Handler().ServeHTTP(rec, req)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello") {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	if a.n != 1 || b.n != 1 {
		t.Errorf("asked a %d times and b %d", a.n, b.n)
	}
}
