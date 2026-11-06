package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// upstreamFailed400 is a relay's own word that the provider behind it
// turned the request away: sub2api's answer to a translated Claude Code
// turn whose system message its Codex backend refused (#4632). It is not
// the client's mistake, and not the relay seat's: the group's next member
// is asked, and the seat rests for nothing.
const upstreamFailed400 = `{"error":{"message":"sub2api: Upstream request failed","type":"upstream_error"}}`

// A relay answering 400 "Upstream request failed" hands the request to the
// group's next member, and nobody rests (#4632: the group's second member
// never ran, and every turn died on the first seat). A 400 that says
// nothing of the kind still ends the route, as any bad request would fail
// on every member alike.
func TestGroupFailsOverARelaysUpstreamFailed(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{400, "", upstreamFailed400}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Responses, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 1 || b.n != 1 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 400 || r.Tries[0].Fail != failShape {
		t.Fatalf("tries %+v", r.Tries)
	}
	if rest := r.Tries[0].Rest; rest != nil {
		t.Fatalf("a relay's upstream failure was to rest nobody: %+v", r.Tries[0].Rest)
	}

	// the seat stayed live: the next group request asks it first again
	code, body = postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 2 || b.n != 2 {
		t.Fatalf("next: %d %s (a %d, b %d)", code, body, a.n, b.n)
	}
}

// The words fail a 400 or a 422 over, and nothing beside them: a status of
// the account's own (401, 403, 429), a plain bad request, and a relay that
// also says the quota is spent keep the reading they had.
func TestUpstreamFailedWordsFailARelayOver(t *testing.T) {
	for _, x := range []struct {
		status int
		body   string
		want   bool
	}{
		{400, upstreamFailed400, true},
		// as magpie tells it on, with the relay's name before, and the
		// relay's own type around it; and apex's, whose type said
		// invalid_request_error (#1511)
		{400, `{"error":{"message":"apex: Upstream request failed","type":"invalid_request_error"}}`, true},
		{422, `{"error":{"message":"Upstream request failed"}}`, true},
		// a status of the account's own keeps its reading
		{401, upstreamFailed400, false},
		{403, upstreamFailed400, false},
		{429, upstreamFailed400, false},
		// a plain bad request ends the route, as it would on every member
		{400, `{"error":{"message":"Invalid parameter: model"}}`, false},
		{400, `{"error":{"message":"Upstream request failed, insufficient_quota"}}`, false},
	} {
		if got := shapeRefused(x.status, []byte(x.body)); got != x.want {
			t.Errorf("shapeRefused(%d, %s) = %v, want %v", x.status, x.body, got, x.want)
		}
	}
}
