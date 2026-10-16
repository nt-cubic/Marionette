package gateway

import (
	"cmp"
	"net/http"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

func forgetSinks(t *testing.T) {
	t.Helper()
	clear := func() {
		sunk.Lock()
		sunk.m = map[string]sinking{}
		sunk.Unlock()
		restingUntil.Lock()
		restingUntil.m = map[string]time.Time{}
		restingUntil.note = map[string]Rest{}
		restingUntil.Unlock()
		routed.Lock()
		routed.failures = map[string]int{}
		routed.Unlock()
	}
	clear()
	t.Cleanup(clear)
}

// rested ends what rests by key's rest, as if its minute were up.
func rested(key string) {
	restingUntil.Lock()
	restingUntil.m[key] = time.Now().Add(-time.Second)
	restingUntil.Unlock()
}

func whoOf(cs []candidate) []string {
	var out []string
	for _, c := range cs {
		out = append(out, c.p.Account.User)
	}
	return out
}

// 01huadalang (WorkBuddy, several accounts, in order): an account rate
// limited while it has quota left goes back to the front as soon as its
// rest ends, and is the one a vendor's risk control notices. With Sink on
// it goes to the back instead, behind every one not rate limited since,
// and comes round again only once those ahead of it were rate limited in
// their turn. A used-up account doesn't sink, a resting one still waits
// at the back, and with Sink off the order is as it was.
func TestRateLimitedSinksToTheBack(t *testing.T) {
	forgetSinks(t)
	old := allowances
	defer func() { allowances = old }()
	used := map[string]float64{"a@x.com": 10, "b@x.com": 10, "c@x.com": 10}
	allowances = func(string) map[string]provider.Allowance {
		out := map[string]provider.Allowance{}
		for u, v := range used {
			out[u] = provider.Allowance{{Used: v, Span: 30 * 24 * time.Hour, Resets: time.Now().Add(24 * time.Hour)}}
		}
		return out
	}
	s := &Server{}
	acct := func(user string) candidate {
		return candidate{p: provider.Provider{ID: "workbuddy", Routing: provider.Ordered, Account: &provider.Account{Agent: "workbuddy", User: user}}, model: "m", rest: "workbuddy@" + user}
	}
	cs := []candidate{acct("a@x.com"), acct("b@x.com"), acct("c@x.com")}
	p := provider.Provider{ID: "workbuddy", Routing: provider.Ordered, Sink: true}
	order := func(p provider.Provider) []string {
		out, _ := weigh(p, slices.Clone(cs), "m", "")
		out, _ = restLast(out, planned{order: make([]Weighed, len(out))})
		return whoOf(out)
	}
	rate := []byte(`{"error":{"message":"Too many requests","type":"rate_limit_error"}}`)
	limit := func(i int) {
		if r := s.restAfter(cs[i], 429, http.Header{}, rate); r.Why != failRate {
			t.Fatalf("%s: %s", cs[i].p.Account.User, r.Why)
		}
		rested(cs[i].restKey())
	}
	want := func(p provider.Provider, w ...string) {
		t.Helper()
		if got := order(p); !slices.Equal(got, w) {
			t.Fatalf("order %v, want %v", got, w)
		}
	}
	off := p
	off.Sink = false

	want(p, "a@x.com", "b@x.com", "c@x.com")
	limit(0)
	want(off, "a@x.com", "b@x.com", "c@x.com") // off: back to the first
	want(p, "b@x.com", "c@x.com", "a@x.com")
	limit(1)
	want(p, "c@x.com", "a@x.com", "b@x.com")
	limit(2)
	want(p, "a@x.com", "b@x.com", "c@x.com") // round again: a sank first
	limit(0)
	want(p, "b@x.com", "c@x.com", "a@x.com")

	// in the trace, the one at the back says when it sank
	_, wg := weigh(p, slices.Clone(cs), "m", "")
	if w := weighed(cs[0], p, wg, false, ""); w.Sunk == nil {
		t.Fatal("trace: a sank, and doesn't say so")
	}
	if w := weighed(cs[0], off, wg, false, ""); w.Sunk != nil {
		t.Fatal("trace: sank where the order doesn't sink")
	}

	// in turn goes round already: nothing sinks there
	turn := p
	turn.Routing = provider.Rotate
	if !sinks(p.Sink, p.Routing) || sinks(turn.Sink, turn.Routing) {
		t.Fatal("sinks: in order sinks, in turn doesn't")
	}

	// out of quota, or rate limited with its allowance used up, it doesn't
	// sink: it rests until the quota comes back, and keeps its place
	forgetSinks(t)
	s.restAfter(cs[0], 429, http.Header{}, []byte(`{"error":{"message":"You have hit your usage limit"}}`))
	rested(cs[0].restKey())
	want(p, "a@x.com", "b@x.com", "c@x.com")
	used["a@x.com"] = 100
	s.restAfter(cs[0], 429, http.Header{}, rate)
	used["a@x.com"] = 10
	rested(cs[0].restKey())
	want(p, "a@x.com", "b@x.com", "c@x.com")

	// a sunk one resting is still behind every one ready
	limit(0)
	s.restAfter(cs[1], 429, http.Header{}, rate) // b rests on
	want(p, "c@x.com", "a@x.com", "b@x.com")
}

// A routing group with Sink: a member's key rate limited with quota left
// goes behind the other members, in order or not, and the trace's order
// follows it.
func TestGroupRateLimitedSinks(t *testing.T) {
	forgetSinks(t)
	s := &Server{}
	mem := func(id string) provider.Member {
		return provider.Member{ID: id + "/m", Path: []string{id + "/m"}, Provider: provider.Provider{ID: id, Chat: "http://127.0.0.1:1/v1", Key: "k-" + id}, Model: "m"}
	}
	ms := []provider.Member{mem("a"), mem("b"), mem("c")}
	ids := func(cs []candidate) []string {
		var out []string
		for _, c := range cs {
			out = append(out, c.p.ID)
		}
		return out
	}
	rate := []byte(`{"error":{"message":"Too many requests"}}`)
	cs, _ := s.planGroup(provider.Group{ID: "g", Routing: provider.Ordered}, ms, provider.Chat)
	s.restAfter(cs[0], 429, http.Header{}, rate)
	rested(cs[0].restKey())
	for _, routing := range []string{provider.Ordered, ""} {
		g := provider.Group{ID: "g", Routing: routing, Sink: true}
		out, pl := s.planGroup(g, ms, provider.Chat)
		if got := ids(out); !slices.Equal(got, []string{"b", "c", "a"}) {
			t.Fatalf("%q: order %v", routing, got)
		}
		if pl.order[2].Provider != "a" || pl.order[2].Sunk == nil || pl.order[0].Sunk != nil {
			t.Fatalf("%q: trace %+v", routing, pl.order)
		}
		g.Sink = false
		if out, _ := s.planGroup(g, ms, provider.Chat); ids(out)[0] != "a" {
			t.Fatalf("%q off: order %v", routing, ids(out))
		}
	}
}

// A group in the group with Sink on (#2255, ggbdpq): its key rate limited
// with quota left goes to the back of that group's own order — behind its
// own other keys, not behind the outer group's members, which the outer
// group's Sink alone would do — as it plans by its own routing (#576) and
// rules. Whatever the outer group's routing, at any depth; with the inner
// group's Sink off, and the outer's off, the order is the routing's.
func TestNestedGroupSinksItsOwnOrder(t *testing.T) {
	forgetSinks(t)
	s := &Server{}
	prov := func(id string) provider.Provider {
		return provider.Provider{ID: id, Chat: "http://127.0.0.1:1/v1", Key: "k-" + id}
	}
	// one member of outer: a model it names itself, or one of the
	// groups via, the outermost first, down to the model
	mem := func(id string, via ...provider.Group) provider.Member {
		path := []string{}
		for _, g := range via {
			path = append(path, provider.GroupPrefix+g.ID)
		}
		path = append(path, id+"/m")
		return provider.Member{ID: path[0], Path: path, Via: via, Provider: prov(id), Model: "m"}
	}
	ids := func(cs []candidate) []string {
		var out []string
		for _, c := range cs {
			out = append(out, c.p.ID)
		}
		return out
	}
	first := func(cs []candidate, _ planned) []candidate { return cs }
	rate := []byte(`{"error":{"message":"Too many requests"}}`)
	sinkA := func(t *testing.T) {
		t.Helper()
		forgetSinks(t)
		inner := provider.Group{ID: "inner", Routing: provider.Ordered}
		cs, _ := s.planGroup(provider.Group{ID: "outer", Routing: provider.Ordered}, []provider.Member{mem("a", inner), mem("b", inner)}, provider.Chat)
		if cs[0].p.ID != "a" {
			t.Fatalf("first: %v", ids(cs))
		}
		s.restAfter(cs[0], 429, http.Header{}, rate) // rate limited with quota left: it sank
		rested(cs[0].restKey())                      // and its minute is up
	}
	for _, routing := range []string{provider.Ordered, "", provider.Weighted} {
		t.Run("outer "+cmp.Or(routing, "smart"), func(t *testing.T) {
			sinkA(t)
			inner := provider.Group{ID: "inner", Routing: provider.Ordered, Sink: true}
			outer := provider.Group{ID: "outer", Routing: routing}
			// the inner group first, then a model outer names itself:
			// a goes behind b, next to it (where outer's routing puts
			// the inner group is outer's: by weight it goes round)
			out, pl := s.planGroup(outer, []provider.Member{mem("a", inner), mem("b", inner), mem("x")}, provider.Chat)
			got := ids(out)
			if b := slices.Index(got, "b"); b < 0 || b+1 >= len(got) || got[b+1] != "a" || (routing == provider.Ordered && !slices.Equal(got, []string{"b", "a", "x"})) {
				t.Fatalf("the inner group's own Sink: order %v, want b then a", got)
			}
			for i, w := range pl.order {
				if w.Provider != got[i] || (w.Sunk != nil) != (got[i] == "a") {
					t.Fatalf("trace %+v for order %v", pl.order, got)
				}
			}
			// its Sink off: the routing's order
			inner.Sink = false
			got = ids(first(s.planGroup(outer, []provider.Member{mem("a", inner), mem("b", inner), mem("x")}, provider.Chat)))
			if a := slices.Index(got, "a"); a < 0 || a+1 >= len(got) || got[a+1] != "b" {
				t.Fatalf("Sink off: order %v, want a then b", got)
			}
		})
	}
	t.Run("deeper", func(t *testing.T) {
		sinkA(t)
		// outer → mid (Sink on) → inner (Sink off): mid orders its whole,
		// inner's keys and its own c, and outer's own y stays last
		mid := provider.Group{ID: "mid", Routing: provider.Ordered, Sink: true}
		inner := provider.Group{ID: "inner", Routing: provider.Ordered}
		outer := provider.Group{ID: "outer", Routing: provider.Ordered}
		ms := []provider.Member{mem("a", mid, inner), mem("b", mid, inner), mem("c", mid), mem("y")}
		if out, _ := s.planGroup(outer, ms, provider.Chat); !slices.Equal(ids(out), []string{"b", "c", "a", "y"}) {
			t.Fatalf("mid's Sink: order %v, want [b c a y]", ids(out))
		}
		// inner's Sink on, mid's off: a goes behind b only
		mid.Sink, inner.Sink = false, true
		ms = []provider.Member{mem("a", mid, inner), mem("b", mid, inner), mem("c", mid), mem("y")}
		if out, _ := s.planGroup(outer, ms, provider.Chat); !slices.Equal(ids(out), []string{"b", "a", "c", "y"}) {
			t.Fatalf("inner's Sink: order %v, want [b a c y]", ids(out))
		}
	})
	t.Run("outer only", func(t *testing.T) {
		sinkA(t)
		// the outer group's Sink still orders the whole, as before
		inner := provider.Group{ID: "inner", Routing: provider.Ordered}
		outer := provider.Group{ID: "outer", Routing: provider.Ordered, Sink: true}
		if out, _ := s.planGroup(outer, []provider.Member{mem("a", inner), mem("b", inner), mem("x")}, provider.Chat); !slices.Equal(ids(out), []string{"b", "x", "a"}) {
			t.Fatalf("the outer group's Sink: order %v, want [b x a]", ids(out))
		}
	})
}
