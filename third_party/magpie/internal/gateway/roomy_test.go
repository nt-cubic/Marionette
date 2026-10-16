package gateway

import (
	"net/http/httptest"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// roomyMember saves a provider id serving model m on s, its window
// window tokens (0: not known).
func roomyMember(t *testing.T, id string, window int, s *scripted) {
	t.Helper()
	up := httptest.NewServer(s)
	t.Cleanup(up.Close)
	p := provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Chat: up.URL + "/v1", Models: []string{"m"}}
	if window > 0 {
		p.Contexts = map[string]int{"m": window}
	}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
}

// A group's member whose model is known to take less than the request
// isn't asked it (dgqyushen on Discord: OpenCode Zen's free DeepSeek V4.1
// Flash, 200K, first, and a 1M subscription of it second). Asked, the
// free one only refuses it, and a refusal that doesn't say why, for a
// request not far enough past the window to be taken as too long for it
// (tooLongUnsaid's margin), reached the agent as the vendor's error while
// the next member had room. A request it holds still goes to it first.
func TestMemberTooSmallIsNotAsked(t *testing.T) {
	fresh(t)
	// the free member refuses a request past its window the way some
	// vendors do, without saying it is too long
	unsaid := `{"error":{"message":"Invalid request parameters"}}`
	free := &scripted{replies: []reply{{400, "", unsaid}}}
	paid := &scripted{replies: []reply{{200, "", chatOK}}}
	roomyMember(t, "free", 2000, free)
	roomyMember(t, "paid", 10000, paid)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"free/m", "paid/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	past := strings.Repeat("word ", 1760) // about 2200 tokens as estimated
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"`+past+`"}]}`)
	r := lastRoute(s)
	if code != 200 || !strings.Contains(body, "hello") || free.n != 0 || paid.n != 1 {
		t.Fatalf("past the free member's window: %d %s (free %d, paid %d)", code, body, free.n, paid.n)
	}
	if len(r.Tries) != 1 || r.Tries[0].ID != "paid" {
		t.Fatalf("tries %+v", r.Tries)
	}
	if slices.ContainsFunc(r.Order, func(w Weighed) bool { return w.Provider == "free" }) {
		t.Fatalf("the free member is still in the order: %+v", r.Order)
	}

	// a request it holds goes to it, first
	free.replies = []reply{{200, "", chatOK}}
	free.n, paid.n = 0, 0
	code, body = postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || free.n != 1 || paid.n != 0 {
		t.Fatalf("within the window: %d %s (free %d, paid %d)", code, body, free.n, paid.n)
	}
}

// A conversation that began on the free member, and is kept there by
// affinity, moves once it has grown past that member's window.
func TestConversationOutgrowingItsMemberMoves(t *testing.T) {
	fresh(t)
	free := &scripted{replies: []reply{{200, "", chatOK}}}
	paid := &scripted{replies: []reply{{200, "", chatOK}}}
	roomyMember(t, "free", 2000, free)
	roomyMember(t, "paid", 10000, paid)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"free/m", "paid/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	if code, body := postAs(t, s, "conv", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`); code != 200 || free.n != 1 {
		t.Fatalf("first turn: %d %s (free %d)", code, body, free.n)
	}
	big := strings.Repeat("word ", 2400)
	code, body := postAs(t, s, "conv", `{"model":"group/g","messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"hello"},{"role":"user","content":"`+big+`"}]}`)
	if code != 200 || free.n != 1 || paid.n != 1 {
		t.Fatalf("grown: %d %s (free %d, paid %d)", code, body, free.n, paid.n)
	}
}

// What the check leaves alone: a member whose window isn't known, a
// request no member holds (the first is asked, and the agent told it
// overflowed, to compact), and one whose size can't be estimated.
func TestRoomyLeavesWhatItCantJudge(t *testing.T) {
	fresh(t)
	over := `{"error":{"message":"This model's maximum context length is 2000 tokens","code":"context_length_exceeded"}}`
	unknown := &scripted{replies: []reply{{200, "", chatOK}}}
	small := &scripted{replies: []reply{{400, "", over}}}
	smaller := &scripted{replies: []reply{{400, "", over}}}
	roomyMember(t, "unknown", 0, unknown)
	roomyMember(t, "small", 2000, small)
	roomyMember(t, "smaller", 1500, smaller)
	for _, g := range []provider.Group{
		{Name: "Unknown", Members: []string{"unknown/m", "small/m"}, Routing: provider.Ordered},
		{Name: "None", Members: []string{"small/m", "smaller/m"}, Routing: provider.Ordered},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	s := New()
	big := strings.Repeat("word ", 2400)
	code, body := postAs(t, s, "", `{"model":"group/unknown","messages":[{"role":"user","content":"`+big+`"}]}`)
	if code != 200 || unknown.n != 1 || small.n != 0 {
		t.Fatalf("window not known: %d %s (unknown %d, small %d)", code, body, unknown.n, small.n)
	}
	code, body = postAs(t, s, "", `{"model":"group/none","messages":[{"role":"user","content":"`+big+`"}]}`)
	if code != 400 || !strings.Contains(body, "context_length_exceeded") || small.n != 1 || smaller.n != 0 {
		t.Fatalf("no member holds it: %d %s (small %d, smaller %d)", code, body, small.n, smaller.n)
	}

	// a body that doesn't parse is no estimate: every candidate is left
	cs := []candidate{
		{p: provider.Provider{ID: "small", Contexts: map[string]int{"m": 2000}}, model: "m", rest: "small"},
		{p: provider.Provider{ID: "big", Contexts: map[string]int{"m": 10000}}, model: "m", rest: "big"},
	}
	pl := planned{order: []Weighed{{ID: "small"}, {ID: "big"}}}
	got, gotPl := roomy(cs, pl, provider.Chat, []byte(`{"messages":[`+strings.Repeat(`"word `, 4000)))
	if len(got) != 2 || len(gotPl.order) != 2 {
		t.Fatalf("unparsed: %d candidates, order %+v", len(got), gotPl.order)
	}
	// and one that does leaves the small one out, its order with it
	got, gotPl = roomy(cs, pl, provider.Chat, []byte(`{"model":"g","messages":[{"role":"user","content":"`+big+`"}]}`))
	if len(got) != 1 || got[0].rest != "big" || len(gotPl.order) != 1 || gotPl.order[0].ID != "big" {
		t.Fatalf("parsed: %+v, order %+v", got, gotPl.order)
	}
}
