package gateway

import (
	"fmt"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #3453 (hipposonCN): a rule sends every request of a conversation to a
// member whose model refuses the conversation as too long; another member
// takes it, and the next request tries the first all over again — twice
// within one turn of the report's trace. What refused the conversation as
// too long is put behind the member that answered it, for that
// conversation alone: an overflow is the request's length, not the
// account's, so nobody rests — and another conversation, or this one made
// smaller by its agent's compacting, finds the member where the rule put
// it, the size may hold now.
func TestOverflowRefusedOnceIsAskedLast(t *testing.T) {
	fresh(t)
	over := `{"error":{"message":"prompt is too long: 674232 tokens > 200000 token maximum","type":"invalid_request_error"}}`
	a := &scripted{replies: []reply{{400, "", over}}}
	b := &scripted{replies: []reply{{200, "", chatOK}, {200, "", chatOK}, {200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Affinity: provider.AffinitySession,
		Rules: []provider.Rule{{Use: "a/m", Tokens: 1}}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	// the conversation as it grew: its first user message, which a ruled
	// group keeps the conversation by (firstWords), and the turns since
	body := `{"model":"group/g","messages":[{"role":"user","content":"say something about this"},{"role":"assistant","content":"what about it"},{"role":"user","content":"all of what follows, at length"}]}`

	// the rule puts a first; it refuses the conversation as too long, and
	// b answers
	code, out := postAs(t, s, "s1", body)
	if code != 200 || !strings.Contains(out, "hello") || a.n != 1 || b.n != 1 {
		t.Fatalf("first: %d %s (a %d, b %d)", code, out, a.n, b.n)
	}
	r := lastRoute(s)
	if len(r.Tries) != 2 || r.Tries[0].Fail != failOverflow {
		t.Fatalf("first tries: %+v", r.Tries)
	}

	// the next request of the same conversation, the rule heard as it
	// was: b is asked first, what a refused as too long at this size it
	// refuses again — the conversation has only grown
	a.n, b.n = 0, 0
	code, out = postAs(t, s, "s1", body)
	if code != 200 || a.n != 0 || b.n != 1 {
		t.Fatalf("second: %d %s (a %d, b %d)", code, out, a.n, b.n)
	}

	// another conversation begins with a, as the rule says: what a
	// refused was the first conversation's length, not this one's
	a.n, b.n = 0, 0
	code, out = postAs(t, s, "s2", body)
	if code != 200 || a.n != 1 || b.n != 1 {
		t.Fatalf("another conversation: %d %s (a %d, b %d)", code, out, a.n, b.n)
	}

	// the first conversation compacted to less than what a refused, its
	// first user message kept, so it is the same conversation: a is asked
	// as the rule says again, the size holds now, and a answers
	a.n, b.n = 0, 0
	a.replies = []reply{{200, "", chatOK}}
	code, out = postAs(t, s, "s1", `{"model":"group/g","messages":[{"role":"user","content":"say something about this"}]}`)
	if code != 200 || a.n != 1 || b.n != 0 {
		t.Fatalf("compacted: %d %s (a %d, b %d)", code, out, a.n, b.n)
	}

	// what a answered, it holds: its refusal is forgotten, so the
	// conversation grown back to the size a once refused begins with a
	// again, as the rule says
	a.n, b.n = 0, 0
	a.replies = []reply{{400, "", over}}
	code, out = postAs(t, s, "s1", body)
	if code != 200 || a.n != 1 || b.n != 1 {
		t.Fatalf("after a answered: %d %s (a %d, b %d)", code, out, a.n, b.n)
	}
}

// Two requests of one conversation run at once (a turn and its title, a
// retry beside the original): one remembers a refusal while the other
// orders its members by what is remembered. Under -race the order reads
// what is remembered while it is written.
func TestOverflowMemoryReadWhileNoted(t *testing.T) {
	fresh(t)
	cs := []candidate{{p: provider.Provider{ID: "a"}, model: "m"}, {p: provider.Provider{ID: "b"}, model: "m"}}
	pl := planned{order: make([]Weighed, len(cs))}
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < 2000; i++ {
			noteOverflow("conv", candidate{p: provider.Provider{ID: "a"}, model: fmt.Sprint(i % 7)}, 10)
			forgetOverflow("conv", candidate{p: provider.Provider{ID: "a"}, model: fmt.Sprint(i % 5)})
		}
	}()
	noteOverflow("conv", cs[0], 10)
	for i := 0; i < 2000; i++ {
		behindOverflow("conv", cs, pl, 20)
	}
	<-done
}
