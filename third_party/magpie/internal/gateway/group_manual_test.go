package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A manual group (#317) sends every request to the member the user picked,
// never to the others — not when a rule matches, not when the pick fails —
// and picking another moves the next request there.
func TestManualGroup(t *testing.T) {
	fresh(t)
	a, b := &keyed{}, &keyed{fail: map[string]int{"kb": 500}}
	serveOn(t, "a", "ka", []string{"m"}, a)
	serveOn(t, "b", "kb", []string{"n"}, b)
	g := provider.Group{Name: "Mine", Members: []string{"a/m", "b/n"}, Routing: provider.Manual, Pick: "b/n",
		Rules: []provider.Rule{{Use: "a/m", Tokens: 1}}}
	if err := provider.SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	s := New()
	const req = `{"model":"group/mine","messages":[{"role":"user","content":"hi"}]}`
	code, body := postAs(t, s, "", req)
	if code == 200 || len(a.tried) != 0 || len(b.tried) == 0 || strings.Trim(strings.Join(b.tried, ","), "kb,") != "" {
		t.Fatalf("picked b/n: %d %s, a tried %v, b tried %v", code, body, a.tried, b.tried)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if r.Group == nil || r.Group.Routing != provider.Manual || len(r.Order) != 1 || r.Order[0].Provider != "b" {
		t.Fatalf("trace: %+v", r)
	}

	got, _, _ := provider.FindGroup("group/mine")
	got.Pick = "a/m"
	if err := provider.SaveGroup(got); err != nil {
		t.Fatal(err)
	}
	b.tried = nil
	code, body = postAs(t, s, "", req)
	if code != 200 || !strings.Contains(body, "from ka") || len(b.tried) != 0 {
		t.Fatalf("picked a/m: %d %s, b tried %v", code, body, b.tried)
	}
	// the rules wait while it is manual, but are kept
	for _, x := range provider.Groups() {
		if x.ID == "mine" && len(x.Rules) != 1 {
			t.Fatalf("rules lost: %+v", x)
		}
	}
}
