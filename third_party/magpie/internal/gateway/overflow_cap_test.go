package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A relay's own cap on one request's input is a refusal over the input's
// size, like a model's window: the next member of a group, another vendor,
// may hold it (#2445, worksyang: B.ai's "Input token exceed the limit" on a
// 380K request, under the model's 1M window, failed the whole group, its
// other three members never asked). It goes the way an overflow goes
// (#700): the next member with room is asked, nobody rests, and another
// account of the same model isn't. A request the vendor can't read still
// isn't asked of anyone else, nor is a rate limit worded with input tokens
// taken for one.
func TestRelayInputCapGoesToTheNextMember(t *testing.T) {
	// the vendor's words as the reporter's routing log has them, after
	// magpie's "B.ai: "; the JSON around them is not the reporter's
	const capped = "Input token exceed the limit (request id: 2026…)"
	if !tooLong(400, "B.ai: "+capped) {
		t.Errorf("%q isn't taken for a request too long", capped)
	}
	if !overflowed(400, []byte(`{"error":{"message":"`+capped+`","type":"invalid_request_error"}}`)) {
		t.Errorf("%q in an error body isn't an overflow", capped)
	}
	for _, msg := range []string{
		"Input token exceed the limit per minute",
		"Input tokens exceeded the limit, please wait",
		"input tokens exceed the TPM limit",
	} {
		if tooLong(400, msg) {
			t.Errorf("a rate limit %q taken for a request too long", msg)
		}
	}

	fresh(t)
	a := &scripted{replies: []reply{{400, "", `{"error":{"message":"` + capped + `"}}`}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "Top", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/top","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 1 || b.n != 1 {
		t.Fatalf("capped: %d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 400 || r.Tries[0].Fail != failOverflow || r.Tries[0].Rest != nil || r.Tries[1].Rest != nil {
		t.Fatalf("tries %+v", r.Tries)
	}

	// a plain bad request is as bad at the next member: not asked
	bad := `{"error":{"message":"Invalid parameter: messages","type":"invalid_request_error"}}`
	a.replies, a.n, b.n = []reply{{400, "", bad}}, 0, 0
	code, body = postAs(t, s, "", `{"model":"group/top","messages":[{"role":"user","content":"hi"}]}`)
	if code != 400 || !strings.Contains(body, "Invalid parameter") || a.n != 1 || b.n != 0 {
		t.Fatalf("bad request: %d %s (a %d, b %d)", code, body, a.n, b.n)
	}
}
