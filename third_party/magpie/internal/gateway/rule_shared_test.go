package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// sharedGroups saves davisjiahao's groups (#2361): First and Second share
// their models in opposite orders, and Parent's rule sends every request
// to Second, the later of the two. second is Second's members and rules.
func sharedGroups(t *testing.T, second provider.Group) {
	t.Helper()
	if err := provider.SaveGroup(provider.Group{Name: "First", Members: []string{"a/alpha", "b/beta"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	second.Name, second.Routing = "Second", provider.Ordered
	if err := provider.SaveGroup(second); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Parent", Members: []string{"group/first", "group/second"}, Routing: provider.Ordered,
		Patience: -1, Rules: []provider.Rule{{Use: "group/second", Tokens: 1}}}); err != nil {
		t.Fatal(err)
	}
}

// A parent's rule naming a group in it that shares its models with a
// group ahead of it sends the turn to that group, in that group's own
// order, and fails over in that order; each model is still asked once
// (#2361: Second came out unready and First's Alpha answered, Beta never
// asked).
func TestRuleNamesAGroupSharingModelsWithOneAhead(t *testing.T) {
	for _, failBeta := range []bool{false, true} {
		fresh(t)
		a, b := &keyed{}, &keyed{fail: map[string]int{}}
		if failBeta {
			b.fail["kb"] = 402
		}
		serveOn(t, "a", "ka", []string{"alpha"}, a)
		serveOn(t, "b", "kb", []string{"beta"}, b)
		sharedGroups(t, provider.Group{Members: []string{"b/beta", "a/alpha"}})
		s := New()
		code, body := postAs(t, s, "one", `{"model":"group/parent","messages":[{"role":"user","content":"hello"}]}`)
		r := lastRoute(s)
		want, wantTries := "from kb", 1
		if failBeta {
			want, wantTries = "from ka", 2
		}
		if code != 200 || !strings.Contains(body, want) || len(b.tried) != 1 || len(r.Tries) != wantTries || r.Tries[0].Model != "beta" {
			t.Fatalf("beta failing %v: %d %s tries %+v (a %v, b %v)", failBeta, code, body, r.Tries, a.tried, b.tried)
		}
		if r.Rule == nil || r.Rule.Use != "group/second" || r.Rule.Unready || r.Rule.Instead != "" {
			t.Fatalf("beta failing %v: rule %+v", failBeta, r.Rule)
		}
		// both failing: each model once, not once for each group
		if failBeta {
			a.fail = map[string]int{"ka": 402}
			postAs(t, s, "two", `{"model":"group/parent","messages":[{"role":"user","content":"hi"}]}`)
			if r := lastRoute(s); len(r.Tries) > 2 {
				t.Fatalf("a model asked twice: %+v", r.Tries)
			}
		}
	}
}

// A model only some of whose places are shared: the rule's group has its
// own first, then its own others, then the parent's rest.
func TestRuleNamesAGroupSharingSomeModels(t *testing.T) {
	fresh(t)
	a, b, c := &keyed{}, &keyed{fail: map[string]int{"kb": 402}}, &keyed{fail: map[string]int{"kc": 402}}
	serveOn(t, "a", "ka", []string{"alpha"}, a)
	serveOn(t, "b", "kb", []string{"beta"}, b)
	serveOn(t, "c", "kc", []string{"gamma"}, c)
	sharedGroups(t, provider.Group{Members: []string{"b/beta", "c/gamma"}})
	s := New()
	code, body := postAs(t, s, "one", `{"model":"group/parent","messages":[{"role":"user","content":"hello"}]}`)
	r := lastRoute(s)
	var got []string
	for _, x := range r.Tries {
		got = append(got, x.Model)
	}
	if code != 200 || !strings.Contains(body, "from ka") || strings.Join(got, " ") != "beta gamma alpha" || r.Rule.Unready {
		t.Fatalf("%d %s tries %v rule %+v", code, body, got, r.Rule)
	}
}

// The group the rule chose decides among its models by its own rules,
// though the group ahead of it has them too, and a tool round of the turn
// stays where the turn began.
func TestRuleGroupSharingModelsKeepsItsRulesAndTurn(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"alpha"}, a)
	serveOn(t, "b", "kb", []string{"beta"}, b)
	sharedGroups(t, provider.Group{Members: []string{"b/beta", "a/alpha"}, Rules: []provider.Rule{{Use: "a/alpha", Tokens: 1}}})
	s := New()
	post := func(body string) (string, Route) {
		t.Helper()
		code, out := postAs(t, s, "sess", body)
		if code != 200 {
			t.Fatalf("%d %s", code, out)
		}
		return out, lastRoute(s)
	}
	out, r := post(strings.Replace(chat("hello", nil, 0, ""), "group/r", "group/parent", 1))
	if !strings.Contains(out, "from ka") || r.Rule.Unready || len(r.Nested) != 1 || r.Nested[0].Group != "second" || r.Nested[0].Rule == nil || r.Nested[0].Rule.Use != "a/alpha" || r.Nested[0].Rule.Unready {
		t.Fatalf("turn: %s rule %+v nested %+v", out, r.Rule, r.Nested)
	}
	out, r = post(strings.Replace(chat("hello", nil, 1, ""), "group/r", "group/parent", 1))
	if !strings.Contains(out, "from ka") || !r.Rule.Held || r.Rule.Unready || len(r.Nested) != 1 || r.Nested[0].Group != "second" {
		t.Fatalf("tool round: %s rule %+v nested %+v", out, r.Rule, r.Nested)
	}
}
