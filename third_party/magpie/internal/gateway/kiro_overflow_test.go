package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// What the Kiro plugin answers from 0.1.24 for a conversation past the
// model's window, each way Kiro says it (an exception in its event stream,
// a reply stopped with MODEL_CONTEXT_WINDOW_EXCEEDED), is read as an
// overflow: another member with room answers, the Kiro account doesn't
// rest and isn't asked again, and with no member left the agent is told
// context_length_exceeded, so it compacts (ergou0954_54426 on Discord:
// Kiro's models beside DeepSeek in intent routing, "kiro 那边报错说超出上下文").
// Before 0.1.24 the stream's exception was a 502 the gateway took for a
// passing vendor fault (TestKiroMoverHasTheTooLongFix holds the min).
// The member ids are neutral: kiro is a retired built-in the gateway hands
// to the plugin host.
func TestKiroPluginOverflowGoesToAMemberWithRoom(t *testing.T) {
	for _, msg := range []string{
		"input is too long for the model's context: ValidationException: Input is too long. (CONTENT_LENGTH_EXCEEDS_THRESHOLD)",
		"input is too long for the model's context: the reply stopped as the model's context window was full (MODEL_CONTEXT_WINDOW_EXCEEDED)",
	} {
		fresh(t)
		over := `{"type":"error","error":{"type":"invalid_request_error","message":"` + msg + `"}}`
		kiro := &scripted{replies: []reply{{400, "application/json", over}}}
		other := &scripted{replies: []reply{{200, "", chatOK}}}
		scriptedOn(t, "k", provider.Chat, kiro)
		scriptedOn(t, "d", provider.Chat, other)
		if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"k/m", "d/m"}, Routing: provider.Ordered}); err != nil {
			t.Fatal(err)
		}
		s := New()
		code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 || !strings.Contains(body, "hello") || kiro.n != 1 || other.n != 1 {
			t.Fatalf("%s\ngroup: %d %s (kiro %d, other %d)", msg, code, body, kiro.n, other.n)
		}
		r := s.trace.routes[len(s.trace.routes)-1]
		if len(r.Tries) != 2 || r.Tries[0].Fail != failOverflow || r.Tries[0].Rest != nil {
			t.Fatalf("%s\ntries %+v", msg, r.Tries)
		}

		// alone, the agent is told it overflowed, asked once
		kiro.n = 0
		code, body = postAs(t, s, "", `{"model":"k/m","messages":[{"role":"user","content":"hi"}]}`)
		if code != 400 || !strings.Contains(body, "context_length_exceeded") || kiro.n != 1 {
			t.Fatalf("%s\nalone: %d %s (kiro %d)", msg, code, body, kiro.n)
		}
	}
}
