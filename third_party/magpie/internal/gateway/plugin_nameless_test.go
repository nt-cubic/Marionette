package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The Qoder plugin's 502 for a reply whose only answer was a native tool
// call the model never named, as its errorResponse sends it once its own
// second ask came back the same (magpie-community/plugins#579, packages/
// qoder's NAMELESS): qfmodel's glitch, which the next ask mostly isn't.
const namelessSaid = `{"error":{"message":"the model's native tool call has no name; it is not sent on as a tool","type":"qoder_error","code":502}}`

// A nameless call fails over as a cut does, and nobody rests: the account
// did nothing a minute of backoff would mend (plugins#579: it was rested
// for fallbackCooldown, and the user's next turn skipped it).
func TestPluginNamelessCallFailsOverNotRested(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{502, "application/json", namelessSaid}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", chatStreamSaid("from b")}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from b") || b.n != 1 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	for i, try := range r.Tries[:len(r.Tries)-1] {
		if try.Fail != failCut || try.Rest != nil {
			t.Fatalf("try %d: %+v", i, try)
		}
	}
	if s.resting("a") || s.resting("b") || len(routed.failures) != 0 {
		t.Fatalf("a nameless call is nobody's fault: a resting %v, b %v, failures %v", s.resting("a"), s.resting("b"), routed.failures)
	}

	// the next request asks a first again
	a.replies = []reply{{200, "text/event-stream", chatStreamSaid("from a")}}
	code, body = postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from a") || b.n != 1 {
		t.Fatalf("next request: %d %s (a %d, b %d)", code, body, a.n, b.n)
	}
}

// A 502 that is not the plugin's nameless call still rests the account
// as before.
func TestPluginOrdinary502StillRests(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{502, "application/json", `{"error":{"message":"upstream connect error","type":"qoder_error","code":502}}`}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", chatStreamSaid("from b")}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from b") {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	if !s.resting("a") || s.resting("b") {
		t.Fatalf("an ordinary 502 rests its account: a %v, b %v", s.resting("a"), s.resting("b"))
	}
}

// The words alone decide it, in a body or a stream's error event, and
// words that are near them are not them.
func TestPluginNamelessWords(t *testing.T) {
	for _, x := range []struct {
		status int
		body   string
		want   string
	}{
		{502, namelessSaid, failCut},
		{200, `data: {"error":{"message":"the model's native tool call has no name; it is not sent on as a tool","code":502}}`, failCut},
		{502, `{"error":{"message":"tool call has no name"}}`, failOther},
		{502, `{"error":{"message":"the tool has no name"}}`, failOther},
	} {
		if got := failure(x.status, []byte(x.body)); got != x.want {
			t.Errorf("failure(%d, %s) = %s, want %s", x.status, x.body, got, x.want)
		}
	}
}
