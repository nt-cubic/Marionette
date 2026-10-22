package provider

import (
	"encoding/json"
	"strings"
	"testing"
)

// #2633: a rule's useEffort is one of Efforts, saved lowercased; anything
// else is refused when the group is saved, and a pause takes none.
func TestCleanRulesUseEffort(t *testing.T) {
	members := []string{"a/m", "b/big"}
	got, err := cleanRules([]Rule{{Use: "a/m", Tokens: 10, UseEffort: " LOW "}, {Use: "b/big", Images: true}}, members)
	if err != nil || got[0].UseEffort != "low" || got[1].UseEffort != "" {
		t.Fatalf("%+v %v", got, err)
	}
	b, _ := json.Marshal(got)
	if !strings.Contains(string(b), `"useEffort":"low"`) || strings.Count(string(b), "useEffort") != 1 {
		t.Fatal(string(b))
	}
	for _, tc := range []struct {
		rule Rule
		err  string
	}{
		{Rule{Use: "a/m", Tokens: 10, UseEffort: "huge"}, "one of low, medium, high, xhigh, max"},
		{Rule{Use: "a/m", Tokens: 10, UseEffort: "on"}, "not \"on\""},
		{Rule{Use: "a/m", Tokens: 10, UseEffort: "minimal"}, "not \"minimal\""},
		{Rule{Use: "a/m", Pause: true, Agents: []string{"codex"}, UseEffort: "low"}, "a pause sends nothing"},
	} {
		if _, err := cleanRules([]Rule{tc.rule}, members); err == nil || !strings.Contains(err.Error(), tc.err) {
			t.Errorf("%+v: %v, want …%s", tc.rule, err, tc.err)
		}
	}
}

// Typed: use-effort=<level>, read back the same; a bad level or a pause
// with one is refused.
func TestParseRuleUseEffort(t *testing.T) {
	g := Group{ID: "g", Members: []string{"zhipu/glm-5.3", "kimi/k3"}, Classifier: "zhipu/glm-5.3"}
	r, _, _, err := ParseRule(g, RuleWords(`use=k3 use-effort=High intent="simple questions"`))
	if err != nil || r.Use != "kimi/k3" || r.UseEffort != "high" {
		t.Fatalf("%+v %v", r, err)
	}
	if got := r.Line(); got != `use=kimi/k3 use-effort=high intent="simple questions"` {
		t.Fatal(got)
	}
	again, _, _, err := ParseRule(g, RuleWords(r.Line()))
	if err != nil || again.UseEffort != "high" {
		t.Fatalf("%+v %v", again, err)
	}
	for _, bad := range []string{"use=k3 use-effort=huge tokens=1k", "use=k3 use-effort=on tokens=1k", "pause=k3 use-effort=low time=09:00-10:00"} {
		if _, _, _, err := ParseRule(g, RuleWords(bad)); err == nil {
			t.Errorf("%s parsed", bad)
		}
	}
}
