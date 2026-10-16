package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A model's id without a provider in it that names a routing group is the
// group's: the same members, in the same order, the next taking a request
// the one before can't, as "group/<id>" is. However a vendor spells the
// model, its found group is what it names. With no group, it is one
// provider's as before, and "provider/model" is always that provider's.
func TestBareIDIsTheGroups(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{fail: map[string]int{"kb": 429}}
	serveOn(t, "a", "ka", []string{"gpt-6-sol", "grok-4.7", "only-a"}, a)
	serveOn(t, "b", "kb", []string{"gpt-6-sol", "grok-4.7"}, b)
	if err := provider.SaveGroup(provider.Group{ID: "gpt-6-sol", Name: "Sol", Members: []string{"b/gpt-6-sol", "a/gpt-6-sol"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	ask := func(model string) (int, string, Call, *Route) {
		t.Helper()
		a.tried, b.tried = nil, nil
		restingUntil.Lock()
		restingUntil.m = map[string]time.Time{} // b is tried again each time
		restingUntil.Unlock()
		sticks.Lock()
		sticks.m = map[string]stick{} // and no conversation stays with a
		sticks.Unlock()
		code, body := postAs(t, s, "", `{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`)
		return code, body, s.Recent()[0], s.trace.routes[len(s.trace.routes)-1]
	}

	for _, id := range []string{"gpt-6-sol", "GPT-6-sol", "group/gpt-6-sol"} {
		code, body, c, r := ask(id)
		if code != 200 || !strings.Contains(body, "from ka") || strings.Join(b.tried, ",") != "kb" {
			t.Fatalf("%s: %d %s, b tried %v", id, code, body, b.tried)
		}
		if c.Model != id || c.Provider != "a" || c.Fallback == "" {
			t.Fatalf("%s: call %+v", id, c)
		}
		if r.Model != id || r.Group == nil || r.Group.ID != "gpt-6-sol" || len(r.Order) != 2 || r.Order[0].Provider != "b" {
			t.Fatalf("%s: route %+v", id, r)
		}
	}

	// the found group of grok-4.7, by its id's spelling or the vendors'
	for _, id := range []string{"grok-4-7", "grok-4.7"} {
		code, _, _, r := ask(id)
		if code != 200 || r.Group == nil || r.Group.ID != "auto-grok-4-7" {
			t.Fatalf("%s: %d %+v", id, code, r.Group)
		}
	}

	// a provider's own model is that provider's
	code, _, c, r := ask("b/gpt-6-sol")
	if code != 429 || r.Group != nil || c.Provider != "b" || len(a.tried) != 0 {
		t.Fatalf("b/gpt-6-sol: %d %+v %v", code, r.Group, a.tried)
	}
	// and a model in no group is the one provider's that serves it
	code, _, c, r = ask("only-a")
	if code != 200 || r.Group != nil || c.Provider != "a" {
		t.Fatalf("only-a: %d %+v %s", code, r.Group, c.Provider)
	}

	// a found group the user removed names nothing: the model is one
	// provider's again, and only as a vendor spells it
	if err := provider.DeleteGroup("auto-grok-4-7"); err != nil {
		t.Fatal(err)
	}
	if code, _, c, r = ask("grok-4.7"); code != 200 || r.Group != nil || c.Provider != "a" {
		t.Fatalf("grok-4.7 without its group: %d %+v %s", code, r.Group, c.Provider)
	}
	if code, _ := postAs(t, s, "", `{"model":"grok-4-7","messages":[{"role":"user","content":"hi"}]}`); code != 404 {
		t.Fatalf("grok-4-7 without its group: %d", code)
	}
}
