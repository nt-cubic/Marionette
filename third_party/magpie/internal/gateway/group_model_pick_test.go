package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// unpickAX is #3911's case (timonwong): providers a (x, y) and b (x),
// and the group made of them, then x unchecked in a's model list.
func unpickAX(t *testing.T, g provider.Group) (a, b *keyed) {
	t.Helper()
	fresh(t)
	a, b = &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"x", "y"}, a)
	serveOn(t, "b", "kb", []string{"x"}, b)
	if err := provider.SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModels("a", []string{"y"}); err != nil {
		t.Fatal(err)
	}
	return a, b
}

// A model unchecked in its provider's list is no longer a group's to
// route to (#3911): the Routing page strikes it, and no request goes to
// it, whatever the group's routing.
func TestGroupSkipsModelRemovedFromProviderPicks(t *testing.T) {
	for _, routing := range []string{provider.Ordered, provider.Weighted, provider.Rotate} {
		t.Run(routing, func(t *testing.T) {
			g := provider.Group{ID: "mine", Name: "Mine", Members: []string{"a/x", "b/x"}, Routing: routing}
			if routing == provider.Weighted {
				g.Weights = map[string]int{"a/x": 1000} // a/x would take nearly every request
			}
			a, b := unpickAX(t, g)
			for range 4 {
				code, body := postAs(t, New(), "", `{"model":"group/mine","messages":[{"role":"user","content":"hi"}]}`)
				if code != 200 || !strings.Contains(body, "from kb") {
					t.Fatalf("group/mine after a/x was unchecked: %d %s", code, body)
				}
			}
			if len(a.tried) != 0 || len(b.tried) != 4 {
				t.Fatalf("a tried %v, b tried %v", a.tried, b.tried)
			}
		})
	}
}

// A group in a group whose only model was unchecked has none left, and
// the outer group goes on to its next.
func TestNestedGroupSkipsModelRemovedFromProviderPicks(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"x", "y"}, a)
	serveOn(t, "b", "kb", []string{"x"}, b)
	if err := provider.SaveGroup(provider.Group{ID: "inner", Name: "Inner", Members: []string{"a/x"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "outer", Name: "Outer", Members: []string{"group/inner", "b/x"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModels("a", []string{"y"}); err != nil {
		t.Fatal(err)
	}
	code, body := postAs(t, New(), "", `{"model":"group/outer","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from kb") || len(a.tried) != 0 {
		t.Fatalf("group/outer after a/x was unchecked: %d %s, a tried %v, b tried %v", code, body, a.tried, b.tried)
	}
	if _, ms, ok := provider.FindGroup("group/inner"); !ok || len(ms) != 0 {
		t.Fatalf("inner's members: %v %v", ok, ms)
	}
}

// A request naming the unchecked model itself still reaches it: a
// "provider/model" id goes to a provider that is on whether or not its
// list has the model (resolveIn), and #3911 changed only groups.
func TestUncheckedModelNamedDirectlyStillServed(t *testing.T) {
	a, b := unpickAX(t, provider.Group{ID: "mine", Name: "Mine", Members: []string{"a/x", "b/x"}, Routing: provider.Ordered})
	code, body := postAs(t, New(), "", `{"model":"a/x","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from ka") || len(a.tried) != 1 || len(b.tried) != 0 {
		t.Fatalf("a/x named: %d %s, a tried %v, b tried %v", code, body, a.tried, b.tried)
	}
}

// Checked again, the model is the group's again, where it was.
func TestGroupTakesModelCheckedAgain(t *testing.T) {
	a, b := unpickAX(t, provider.Group{ID: "mine", Name: "Mine", Members: []string{"a/x", "b/x"}, Routing: provider.Ordered})
	if err := provider.SetModels("a", []string{"x", "y"}); err != nil {
		t.Fatal(err)
	}
	code, body := postAs(t, New(), "", `{"model":"group/mine","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "from ka") || len(b.tried) != 0 {
		t.Fatalf("group/mine with a/x checked again: %d %s, a tried %v, b tried %v", code, body, a.tried, b.tried)
	}
}

// A decision model is never in the catalog, picked or not: a decision
// group keeps one whose provider has picks (#3911's check passes it by).
func TestDecisionGroupKeepsAPickedDecider(t *testing.T) {
	fresh(t)
	for _, p := range []provider.Provider{
		{ID: "ja", Name: "JA", Key: "ka", Decide: "http://127.0.0.1:1/v1", Models: []string{"jev-latest"}},
		{ID: "jb", Name: "JB", Key: "kb", Decide: "http://127.0.0.1:1/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "jevs", Name: "Jevs", Members: []string{"ja/jev-latest", "jb/jev-latest"}}); err != nil {
		t.Fatal(err)
	}
	if _, ms, ok := provider.FindGroup("group/jevs"); !ok || len(ms) != 2 {
		t.Fatalf("jevs' members: %v %v", ok, ms)
	}
}
