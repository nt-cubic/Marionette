package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A plugin that reads a model's text-form tool calls ends a turn whose
// stream died mid-call as a failure, so the gateway retries or fails over
// (magpie-community/plugins#38, #2873): a 200 whose stream stops with an
// error event, the call it was reading unsent. The words are the
// contract's own, shared by the plugins that read text-form calls
// (packages/trae, packages/qoder's CUT_SHORT).
const cutShortSaid = "Trae CN: the model's tool call, written as text, was cut short; it is not sent on as the answer"

var cutShortStream = sse(
	`data: {"id":"c","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}`,
	`data: {"error":{"message":"`+cutShortSaid+`","type":"api_error","code":null}}`,
	`data: [DONE]`)

func chatStreamSaid(said string) string {
	return sse(
		`data: {"id":"c","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":"`+said+`"},"finish_reason":"stop"}]}`,
		`data: [DONE]`)
}

// A plugin account whose upstream cut the call short mid-stream is asked
// again — the parser reads the same, the cut doesn't come again — and its
// second answer is the reply; the next member isn't needed, and nobody
// rests: the account did nothing wrong a minute of backoff would mend
// (#2873: the routing record showed fail "other" and a 60s backoff, and
// the retry the plugin engineered never happened).
func TestPluginCutShortIsRetriedNotRested(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{
		{200, "text/event-stream", cutShortStream},
		{200, "text/event-stream", chatStreamSaid("from a")},
	}}
	b := &scripted{replies: []reply{{200, "text/event-stream", chatStreamSaid("from b")}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from a") || strings.Contains(body, "from b") || a.n != 2 || b.n != 0 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Fail != failCut || r.Tries[0].Rest != nil || r.Tries[1].Status != 200 {
		t.Fatalf("tries: %+v", r.Tries)
	}
	if s.resting("a") || s.resting("b") || len(routed.failures) != 0 {
		t.Fatalf("a cut is nobody's fault: a resting %v, b %v, failures %v", s.resting("a"), s.resting("b"), routed.failures)
	}

	// the next request finds the account as good as it was: asked first
	// again, and answering
	code, body = postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from a") || b.n != 0 {
		t.Fatalf("next request: %d %s (a %d, b %d)", code, body, a.n, b.n)
	}
}

// Every member's stream may cut, and the turn still ends: the same one is
// asked again once, the members after it as any failure's are, and the
// agent is told after the retries run out — with nobody resting, so the
// user's next attempt isn't met by a minute of backoff (#2873).
func TestPluginCutShortEveryMemberCutEndsTheTurn(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{200, "text/event-stream", cutShortStream}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", cutShortStream}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	// a streaming agent is told of the failure as its stream's error event
	if code != 200 || !strings.Contains(body, "cut short") {
		t.Fatalf("%d %s", code, body)
	}
	if a.n != 2 || b.n != 1+lastRetries {
		t.Fatalf("a asked %d, b %d", a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if s.resting("a") || s.resting("b") || len(routed.failures) != 0 {
		t.Fatalf("nobody rests for a cut: a %v, b %v, failures %v", s.resting("a"), s.resting("b"), routed.failures)
	}
	for i, try := range r.Tries {
		if try.Rest != nil {
			t.Fatalf("try %d rested: %+v", i, try)
		}
	}
}

// A plugin's other errors keep their own lanes: the contract's unreadable
// call (#2414), a model its vendor doesn't serve, a sign-in it turned
// away — none is a cut, and each rests or fails over as it did.
func TestPluginCutShortLeavesOtherErrorsAlone(t *testing.T) {
	for _, x := range []struct {
		status int
		body   string
		want   string
	}{
		// the contract's UNREADABLE: the block is whole but can't be read
		// — the model's doing, told apart from a stream that died
		{502, `{"error":{"message":"Trae CN: the model's tool call, written as text, can't be read; it is not sent on as the answer","type":"api_error"}}`, failOther},
		// a plugin whose vendor doesn't serve the model asked for
		{400, `{"error":{"message":"the model fake-x does not exist"}}`, failOther},
		// a sign-in its vendor turned away
		{401, `{"error":{"message":"token expired"}}`, failOther},
		// the vendor's own cut-short words, a reply of its own kind
		{502, `{"error":{"message":"upstream connect error"}}`, failOther},
	} {
		if got := failure(x.status, []byte(x.body)); got != x.want {
			t.Errorf("failure(%d, %s) = %s, want %s", x.status, x.body, got, x.want)
		}
	}
	if got := failure(502, []byte(`{"error":{"message":"`+cutShortSaid+`"}}`)); got != failCut {
		t.Errorf("the contract's cut-short error is %s, want %s", got, failCut)
	}
}
