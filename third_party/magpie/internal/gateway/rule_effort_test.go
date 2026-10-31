package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #2633: a rule picks its model and the reasoning that model is asked
// for. The rule's useEffort is what a Chat member is sent, over what the
// agent asked or with nothing asked, fitted to the model's own levels;
// the trace says the rule set it.
func TestRuleSetsItsModelsEffort(t *testing.T) {
	s, a, _ := ruled(t)
	if err := provider.SetModelEfforts("a/small", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct{ use, asked, want string }{
		{"low", "high", "low"},
		{"low", "", "low"},
		{"high", "minimal", "high"},
		{"max", "low", "high"}, // the model's nearest
	} {
		setMembers(t, []string{"b/big", "a/small"}, provider.Rule{Use: "a/small", Tokens: 1000, UseEffort: c.use})
		extra := ""
		if c.asked != "" {
			extra = `,"reasoning_effort":"` + c.asked + `"`
		}
		out, r := postOK(t, s, "", chat(long(2000), nil, 0, extra))
		if sent := sentBody(t, a); !strings.Contains(out, "from ka") || sent["reasoning_effort"] != c.want {
			t.Fatalf("use-effort=%s asked %q: sent %v (%s)", c.use, c.asked, sent, out)
		}
		if r.Rule == nil || r.Rule.UseEffort != c.use || len(r.Tries) != 1 || r.Tries[0].RuleEffort != c.want || r.Tries[0].Effort != c.want {
			t.Fatalf("use-effort=%s asked %q: traced %+v %+v", c.use, c.asked, r.Rule, r.Tries)
		}
	}
}

// The reporter's case: an intent picks the model, and the effort with it;
// the turn's tool rounds keep both.
func TestIntentRuleSetsEffort(t *testing.T) {
	s, _, b, c := intented(t, provider.Rule{Use: "b/big", Intent: "simple questions", UseEffort: "low"})
	if err := provider.SetModelEfforts("b/big", []string{"low", "medium", "high", "xhigh"}); err != nil {
		t.Fatal(err)
	}
	c.set("1", 0)
	for tools := range 2 {
		out, r := postOK(t, s, "s1", chat("what is 2+2?", nil, tools, `,"reasoning_effort":"xhigh"`))
		if sent := sentBody(t, b); !strings.Contains(out, "from kb") || sent["reasoning_effort"] != "low" {
			t.Fatalf("round %d: sent %v (%s)", tools, sent, out)
		}
		if r.Rule == nil || r.Rule.UseEffort != "low" || r.Tries[0].RuleEffort != "low" {
			t.Fatalf("round %d: traced %+v %+v", tools, r.Rule, r.Tries)
		}
	}
}

// Unset, the agent's effort goes as it asked; and a model whose levels
// aren't known is asked as the agent asked, with nothing traced.
func TestRuleEffortUnsetOrUnknownLevels(t *testing.T) {
	s, a, _ := ruled(t)
	setMembers(t, []string{"b/big", "a/small"}, provider.Rule{Use: "a/small", Tokens: 1000, UseEffort: "low"})
	_, r := postOK(t, s, "", chat(long(2000), nil, 0, `,"reasoning_effort":"high"`))
	if sent := sentBody(t, a); sent["reasoning_effort"] != "high" || r.Tries[0].RuleEffort != "" {
		t.Fatalf("no levels: sent %v, traced %+v", sent, r.Tries)
	}
	_, r = postOK(t, s, "", chat(long(2000), nil, 0, ""))
	if sent := sentBody(t, a); sent["reasoning_effort"] != nil || r.Tries[0].RuleEffort != "" {
		t.Fatalf("no levels, none asked: sent %v, traced %+v", sent, r.Tries)
	}
	if err := provider.SetModelEfforts("a/small", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	setMembers(t, []string{"b/big", "a/small"}, provider.Rule{Use: "a/small", Tokens: 1})
	_, r = postOK(t, s, "", chat(long(2000), nil, 0, `,"reasoning_effort":"high"`))
	if sent := sentBody(t, a); sent["reasoning_effort"] != "high" || r.Rule.UseEffort != "" || r.Tries[0].RuleEffort != "" {
		t.Fatalf("unset: sent %v, traced %+v %+v", sent, r.Rule, r.Tries)
	}
	_, r = postOK(t, s, "", chat(long(2000), nil, 0, ""))
	if sent := sentBody(t, a); sent["reasoning_effort"] != nil {
		t.Fatalf("unset, none asked: sent %v", sent)
	}
}

// #3664: an intent names the model and the effort it is asked for, and
// that model's provider turns the request away. The group fails over to
// another of its members, and the turn keeps the intent's effort there,
// fitted to that model's own levels, rather than the agent's. A model whose
// levels aren't known is asked as the agent asked, and a member the user
// fixed at an effort keeps its own (#189).
func TestIntentEffortSurvivesTheGroupsFailover(t *testing.T) {
	for _, x := range []struct {
		name, big   string
		levels      []string // b/big's, nil for unknown
		want, ruled string   // what b is sent, and Try.RuleEffort
		fixed       string
	}{
		{"unknown levels", "b/big", nil, "high", "", ""},
		{"the intent's level", "b/big", []string{"low", "medium", "high"}, "low", "low", ""},
		{"fitted to its own", "b/big", []string{"medium", "high"}, "medium", "medium", ""},
		{"fixed at its own", "b/big:high", []string{"low", "medium", "high"}, "high", "", "high"},
	} {
		// each its own magpie: the first failure rests a/small's key
		s, a, b, c := intented(t, provider.Rule{Use: "a/small", Intent: "simple questions", UseEffort: "low"})
		if err := provider.SetModelEfforts("a/small", []string{"low", "medium", "high"}); err != nil {
			t.Fatal(err)
		}
		if x.levels != nil {
			if err := provider.SetModelEfforts("b/big", x.levels); err != nil {
				t.Fatal(err)
			}
		}
		g, _, _ := provider.FindGroup("group/r")
		g.Members = []string{"a/small", x.big}
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
		c.set("1", 0)
		a.mu.Lock()
		a.fail = 500
		a.mu.Unlock()
		out, r := postOK(t, s, "", chat(long(2000), nil, 0, `,"reasoning_effort":"high"`))
		if sent := sentBody(t, b); !strings.Contains(out, "from kb") || sent["reasoning_effort"] != x.want {
			t.Fatalf("%s: the member the group failed over to sent %v (%s)", x.name, sent, out)
		}
		if sent := sentBody(t, a); sent["reasoning_effort"] != "low" {
			t.Fatalf("%s: the intent's member sent %v", x.name, sent)
		}
		if len(r.Tries) != 2 || r.Tries[0].RuleEffort != "low" || r.Tries[1].RuleEffort != x.ruled || r.Tries[1].Effort != x.want || r.Tries[1].Fixed != x.fixed {
			t.Fatalf("%s: traced %+v", x.name, r.Tries)
		}
	}
}

// #3664 within a group of groups: the inner group's rule asks its member
// for low reasoning. When that member fails, the inner group's other is
// asked low too; when the whole inner group fails, the outer group's
// member outside it is asked as the agent asked, since the inner rule
// decides for its own group's members only.
func TestNestedRuleEffortStaysInItsGroup(t *testing.T) {
	fresh(t)
	k := &keyed{fail: map[string]int{}}
	for _, id := range []string{"f1", "q1", "q2"} {
		serveOn(t, id, "k"+id, []string{"m"}, k)
		if err := provider.SetModelEfforts(id+"/m", []string{"low", "medium", "high"}); err != nil {
			t.Fatal(err)
		}
	}
	for _, g := range []provider.Group{
		{Name: "Quality", Members: []string{"q1/m", "q2/m"}, Routing: provider.Ordered,
			Rules: []provider.Rule{{Use: "q1/m", Tokens: 1, UseEffort: "low"}}},
		{Name: "Coding", Members: []string{"group/quality", "f1/m"}, Routing: provider.Ordered},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	s := New()
	body := `{"model":"group/coding","reasoning_effort":"high","messages":[{"role":"user","content":"fix the bug"}]}`
	send := func(want string) Route {
		t.Helper()
		k.tried = nil
		code, out := postAs(t, s, "", body)
		if code != 200 || !strings.Contains(out, "from k"+want) {
			t.Fatalf("%d %s, tried %v", code, out, k.tried)
		}
		return lastRoute(s)
	}
	efforts := func(r Route) string {
		var out []string
		for _, tr := range r.Tries {
			out = append(out, tr.ID+"="+tr.Effort+"/"+tr.RuleEffort)
		}
		return strings.Join(out, " ")
	}
	k.fail["kq1"] = 500
	if r := send("q2"); efforts(r) != "q1=low/low q2=low/low" {
		t.Fatalf("within the inner group: %s", efforts(r))
	}
	k.fail["kq2"] = 500
	s.UnrestAll("q1")
	if r := send("f1"); efforts(r) != "q1=low/low q2=low/low f1=high/" {
		t.Fatalf("outside it: %s", efforts(r))
	}
}

// An Anthropic member, asked from an Anthropic client, is sent the rule's
// effort as its thinking: a budget for an older Claude.
func TestRuleEffortAnthropicMember(t *testing.T) {
	s, _, _ := ruled(t)
	up := &adaptiveVendor{}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Anthropic: srv.URL, Models: []string{"claude-sonnet-4-5"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModelEfforts("relay/claude-sonnet-4-5", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	setMembers(t, []string{"b/big", "relay/claude-sonnet-4-5"}, provider.Rule{Use: "relay/claude-sonnet-4-5", Tokens: 1000, UseEffort: "low"})
	for _, body := range []string{
		`{"model":"group/r","max_tokens":32000,"thinking":{"type":"enabled","budget_tokens":24000},"messages":[{"role":"user","content":` + quote(long(2000)) + `}]}`,
		`{"model":"group/r","max_tokens":32000,"messages":[{"role":"user","content":` + quote(long(2000)) + `}]}`,
	} {
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body)
		}
		sent := up.last()
		th, _ := sent["thinking"].(map[string]any)
		oc, _ := sent["output_config"].(map[string]any)
		if th["type"] != "enabled" || th["budget_tokens"] != float64(budgetOf("low")) || oc["effort"] != nil {
			t.Fatalf("%s: sent thinking %v output_config %v", body, th, oc)
		}
		if r := lastRoute(s); r.Rule == nil || r.Rule.UseEffort != "low" || r.Tries[0].RuleEffort != "low" {
			t.Fatalf("traced %+v %+v", r.Rule, r.Tries)
		}
	}
}
