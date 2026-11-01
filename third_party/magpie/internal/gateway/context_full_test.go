package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// contextFull is how Kiro-backed relays say a conversation is past the
// model's window: kiro.rs (src/anthropic/handlers.rs, map_provider_error)
// and kiro2claude (error-mapper.ts) turn Kiro's
// CONTENT_LENGTH_EXCEEDS_THRESHOLD into it, as a 400
// invalid_request_error, and a new-api relay in front of one adds its
// request id. ergou0954_54426 on Discord had it from a "kiro opus" model
// of an API-key relay in an intent-routed group: not read as an overflow,
// it went to the agent as a plain 400, which never compacted, and no other
// member was asked.
const contextFull = "Context window is full. Reduce conversation history, system prompt, or tools."

// The words are an overflow on every request-error status a relay sends
// them with, and in a stream's error event; neither a used-up plan nor
// a fault another account of the model would answer differently. An
// ordinary 400 is still not one.
func TestContextWindowFullIsAnOverflow(t *testing.T) {
	said := contextFull + " (request id: 2026110112345678901234567)"
	bodies := []string{
		`{"type":"error","error":{"type":"invalid_request_error","message":"` + contextFull + `"}}`,
		`{"error":{"message":"` + said + `","type":"invalid_request_error","param":"","code":null}}`,
		// its sibling in kiro.rs, for one input past Kiro's own limit
		`{"type":"error","error":{"type":"invalid_request_error","message":"Input is too long. Reduce the size of your messages."}}`,
	}
	for _, status := range []int{400, 413, 422} {
		for _, b := range bodies {
			if !overflowed(status, []byte(b)) {
				t.Errorf("%d %s isn't an overflow", status, b)
			}
			if retryable(status, []byte(b)) {
				t.Errorf("%d %s is asked of another account", status, b)
			}
			if f := failure(status, []byte(b)); f != failOther {
				t.Errorf("%d %s is a %s failure", status, b, f)
			}
		}
	}
	// in a stream, as an error event: an overflow's 400, not a vendor's
	// passing 502 that rests the account and is asked again
	if got := streamStatus(said); got != 400 {
		t.Errorf("a stream's %q stands for %d", said, got)
	}
	if lateRests(said) {
		t.Errorf("a reply cut by %q rests the account", said)
	}
	for _, msg := range []string{
		`{"error":{"message":"invalid model: kiro-opus-9 (request id: 20261101)","type":"invalid_request_error"}}`,
		`{"error":{"message":"model not found","type":"invalid_request_error"}}`,
		`{"error":{"message":"Invalid request: messages is required","type":"invalid_request_error"}}`,
	} {
		if overflowed(400, []byte(msg)) {
			t.Errorf("an ordinary 400 %s is taken for an overflow", msg)
		}
	}
}

// In a group the member saying its context window is full is passed for
// the next, asked once and not rested, both as an error status and as an
// error event of a stream begun with 200; alone, the status tells the
// agent context_length_exceeded, so it compacts.
func TestContextWindowFullGoesToAMemberWithRoom(t *testing.T) {
	said := contextFull + " (request id: 2026110112345678901234567)"
	for _, full := range []reply{
		{400, "application/json", `{"error":{"message":"` + said + `","type":"invalid_request_error","param":"","code":null}}`},
		{200, "text/event-stream", "data: {\"error\":{\"message\":\"" + said + "\",\"type\":\"invalid_request_error\"}}\n\n"},
	} {
		fresh(t)
		relay := &scripted{replies: []reply{full}}
		other := &scripted{replies: []reply{{200, "", chatOK}}}
		scriptedOn(t, "r", provider.Chat, relay)
		scriptedOn(t, "d", provider.Chat, other)
		if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"r/m", "d/m"}, Routing: provider.Ordered}); err != nil {
			t.Fatal(err)
		}
		s := New()
		code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 || !strings.Contains(body, "hello") || relay.n != 1 || other.n != 1 {
			t.Fatalf("%d %s\ngroup: %d %s (relay %d, other %d)", full.code, full.ctype, code, body, relay.n, other.n)
		}
		r := s.trace.routes[len(s.trace.routes)-1]
		if len(r.Tries) != 2 || r.Tries[0].Fail != failOverflow || r.Tries[0].Rest != nil {
			t.Fatalf("%d %s\ntries %+v", full.code, full.ctype, r.Tries)
		}

		// alone, the agent is told it overflowed, asked once. (A stream's
		// error event from the one candidate left reaches the agent as the
		// relay sent it; that is #4675.)
		if full.code != 400 {
			continue
		}
		relay.n = 0
		code, body = postAs(t, s, "", `{"model":"r/m","messages":[{"role":"user","content":"hi"}]}`)
		if code != 400 || !strings.Contains(body, "context_length_exceeded") || relay.n != 1 {
			t.Fatalf("%d %s\nalone: %d %s (relay %d)", full.code, full.ctype, code, body, relay.n)
		}
	}
}
