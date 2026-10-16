package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A model the group names itself that a group in it, listed ahead of it,
// has too is still the one a rule naming it sends the turn to, whatever
// that group's own rules pick (#625: Astra on its own and in a group with
// Opus; a rule for Astra came out unready, and Opus answered).
func TestRuleNamesAModelAGroupInItHasToo(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{}
	serveOn(t, "a", "ka", []string{"m"}, a)
	serveOn(t, "b", "kb", []string{"m"}, b)
	if err := provider.SaveGroup(provider.Group{Name: "Mix", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered,
		Rules: []provider.Rule{{Use: "a/m", Tokens: 1}}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Top", Members: []string{"group/mix", "b/m"}, Routing: provider.Ordered,
		Rules: []provider.Rule{{Use: "b/m", Tokens: 2000}, {Use: "group/mix", Tokens: 1}}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "one", `{"model":"group/top","messages":[{"role":"user","content":`+quote(long(3000))+`}]}`)
	r := lastRoute(s)
	if code != 200 || !strings.Contains(body, "from kb") {
		t.Fatalf("%d %s rule %+v nested %+v", code, body, r.Rule, r.Nested)
	}
	if r.Rule == nil || r.Rule.Use != "b/m" || r.Rule.Unready || r.Rule.Instead != "" {
		t.Fatalf("rule: %+v", r.Rule)
	}

	// the group in it, by its own rule, for the others
	code, body = postAs(t, s, "two", `{"model":"group/top","messages":[{"role":"user","content":"hello"}]}`)
	r = lastRoute(s)
	if code != 200 || !strings.Contains(body, "from ka") || r.Rule == nil || r.Rule.Use != "group/mix" || r.Rule.Unready {
		t.Fatalf("%d %s rule %+v", code, body, r.Rule)
	}
}
