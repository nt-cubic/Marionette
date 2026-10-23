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

// Only the member the rule names: once it fails, the next member is asked
// as the agent asked.
func TestRuleEffortStaysWithItsMember(t *testing.T) {
	s, a, b := ruled(t)
	for _, m := range []string{"a/small", "b/big"} {
		if err := provider.SetModelEfforts(m, []string{"low", "medium", "high"}); err != nil {
			t.Fatal(err)
		}
	}
	setMembers(t, []string{"a/small", "b/big"}, provider.Rule{Use: "a/small", Tokens: 1000, UseEffort: "low"})
	a.mu.Lock()
	a.fail = 500
	a.mu.Unlock()
	out, r := postOK(t, s, "", chat(long(2000), nil, 0, `,"reasoning_effort":"medium"`))
	if sent := sentBody(t, b); !strings.Contains(out, "from kb") || sent["reasoning_effort"] != "medium" {
		t.Fatalf("fallback sent %v (%s)", sent, out)
	}
	if sent := sentBody(t, a); sent["reasoning_effort"] != "low" {
		t.Fatalf("ruled member sent %v", sent)
	}
	if len(r.Tries) != 2 || r.Tries[0].RuleEffort != "low" || r.Tries[1].RuleEffort != "" {
		t.Fatalf("traced %+v", r.Tries)
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
