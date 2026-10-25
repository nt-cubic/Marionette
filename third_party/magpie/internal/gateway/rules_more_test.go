package gateway

import (
	"net/http/httptest"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// withFlash adds c/flash (128k, images) to ruled's providers and gives
// the group the members and rules given.
func withFlash(t *testing.T, members []string, rules ...provider.Rule) *ruleUp {
	t.Helper()
	c := &ruleUp{key: "kc"}
	srv := httptest.NewServer(c)
	t.Cleanup(srv.Close)
	yes := true
	if err := provider.Save(provider.Provider{ID: "c", Name: "C", Key: "kc", Models: []string{"flash"}, Chat: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("c", srv.URL+"/v1", []catalog.Model{{ID: "flash", Context: 128000, Images: true, ImageInput: &yes}}); err != nil {
		t.Fatal(err)
	}
	setMembers(t, members, rules...)
	return c
}

// A rule naming several models (monkeyin_92 on X): the first goes first,
// and when it fails or rests the rule's next one takes the request, ahead
// of the group's own first member; the trace says the rule's next went
// first, the rule's member being ready. The turn's tool rounds stay on it.
func TestRuleSeveralModelsInOrder(t *testing.T) {
	s, a, b := ruled(t)
	c := withFlash(t, []string{"a/small", "b/big", "c/flash"}, provider.Rule{Use: "b/big", More: []string{"c/flash"}, Tokens: 20000})
	b.mu.Lock()
	b.fail = 429
	b.mu.Unlock()
	code, out := postAs(t, s, "one", chat(long(25000), nil, 0, ""))
	r := lastRoute(s)
	if code != 200 || !strings.Contains(out, "from kc") || r.Rule.N != 1 || !slices.Equal(r.Rule.More, []string{"c/flash"}) || len(r.Tries) != 2 {
		t.Fatalf("b failing: %d %s %+v %d", code, out, r.Rule, len(r.Tries))
	}
	b.mu.Lock()
	b.fail = 0
	b.mu.Unlock()
	code, out = postAs(t, s, "two", chat(long(25000), nil, 0, ""))
	r = lastRoute(s)
	if code != 200 || !strings.Contains(out, "from kc") || r.Rule.Unready || r.Rule.Instead != "c/flash" || len(r.Tries) != 1 {
		t.Fatalf("b resting: %d %s %+v %d", code, out, r.Rule, len(r.Tries))
	}
	code, out = postAs(t, s, "two", chat(long(25000), nil, 1, ""))
	r = lastRoute(s)
	if code != 200 || !strings.Contains(out, "from kc") || !r.Rule.Held || r.Rule.Unready || r.Rule.Instead != "c/flash" {
		t.Fatalf("tool round: %d %s %+v", code, out, r.Rule)
	}
	if a.n() != 0 || b.n() != 1 || c.n() != 3 {
		t.Fatalf("a %d b %d c %d", a.n(), b.n(), c.n())
	}

	// the rule's models before the later rules' and the group's others
	setMembers(t, []string{"a/small", "c/flash", "b/big"}, provider.Rule{Use: "b/big", More: []string{"a/small"}, Tokens: 20000},
		provider.Rule{Use: "c/flash", Tokens: 10000})
	_, r = postOK(t, s, "three", chat(long(25000), nil, 0, ""))
	if !slices.Equal(r.Rule.Then, []string{"c/flash"}) || len(r.Tries) != 1 || !strings.HasPrefix(r.Tries[0].ID, "a") || r.Rule.Instead != "a/small" {
		t.Fatalf("order: %+v %+v", r.Rule, r.Tries)
	}
}

// The rule's useEffort is asked of each model it names, the next one too.
func TestRuleEffortReachesTheRulesNextModel(t *testing.T) {
	s, a, b := ruled(t)
	if err := provider.SetModelEfforts("a/small", []string{"low", "medium", "high"}); err != nil {
		t.Fatal(err)
	}
	setMembers(t, []string{"b/big", "a/small"}, provider.Rule{Use: "b/big", More: []string{"a/small"}, Tokens: 1000, UseEffort: "low"})
	b.mu.Lock()
	b.fail = 500
	b.mu.Unlock()
	out, r := postOK(t, s, "", chat(long(2000), nil, 0, `,"reasoning_effort":"high"`))
	if sent := sentBody(t, a); !strings.Contains(out, "from ka") || sent["reasoning_effort"] != "low" {
		t.Fatalf("sent %v (%s)", sent, out)
	}
	if n := len(r.Tries); n != 2 || r.Tries[1].RuleEffort != "low" {
		t.Fatalf("traced %+v", r.Tries)
	}
}

// A compaction longer than a rule's first model takes goes to the rule's
// next one that takes it, by the same rule.
func TestRuleCompactPassesOverTheRulesSmallModel(t *testing.T) {
	s, _, _ := ruled(t, provider.Rule{Use: "a/small", More: []string{"b/big"}, Compact: true})
	ccSystem := `{"role":"system","content":"You are a helpful AI assistant tasked with summarizing conversations."},`
	body := `{"model":"group/r","messages":[` + ccSystem + `{"role":"user","content":` + quote(long(70000)) + `},{"role":"assistant","content":"done"},` +
		`{"role":"user","content":"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests."}]}`
	code, out := postAs(t, s, "big", body)
	r := lastRoute(s)
	if code != 200 || !strings.Contains(out, "from kb") || !r.Rule.Compact || r.Rule.N != 1 || r.Rule.Use != "b/big" || !slices.Equal(r.Rule.Small, []string{"a/small"}) {
		t.Fatalf("%d %s %+v", code, out, r.Rule)
	}
}
