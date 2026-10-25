package provider

import (
	"encoding/json"
	"strings"
	"testing"
)

// A pause rule holds by its hours, days or agents, and only those; it puts
// nobody first, and a group of pause rules alone decides nothing as a turn
// begins.
func TestPauseRuleCleanAndMatch(t *testing.T) {
	peak := &TimeWindow{From: "09:00", To: "18:00", Days: []string{"mon", "tue", "wed", "thu", "fri"}}
	rules, err := cleanRules([]Rule{{Use: "a/x", Pause: true, Time: peak}, {Use: "b/y", Pause: true, Agents: []string{"Codex"}}}, []string{"a/x", "b/y"})
	if err != nil {
		t.Fatal(err)
	}
	for _, bad := range []Rule{
		{Use: "a/x", Pause: true, Tokens: 100},
		{Use: "a/x", Pause: true, Images: true},
		{Use: "a/x", Pause: true, Intent: "tests"},
		{Use: "a/x", Pause: true, Compact: true},
		{Use: "a/x", Pause: true, Effort: "high"},
		{Use: "a/x", Pause: true},
	} {
		if _, err := cleanRules([]Rule{bad}, []string{"a/x"}); err == nil {
			t.Errorf("%+v saved", bad)
		}
	}
	q := RuleRequest{Agent: "codex", At: at(2, 10, 0)}
	if MatchRule(rules, q) != -1 || len(ThenUses(rules, q, 0, nil)) != 0 {
		t.Fatal("a pause rule sent a request to its model")
	}
	if (Group{Rules: rules}).Ruled() || !(Group{Rules: append(rules, Rule{Use: "a/x", Tokens: 1})}).Ruled() {
		t.Fatal("Ruled")
	}
	for _, c := range []struct {
		r     Rule
		agent string
		hour  int
		want  bool
	}{
		{rules[0], "claude", 10, true},
		{rules[0], "claude", 19, false},
		{rules[1], "codex", 19, true},
		{rules[1], "claude", 10, false},
		{Rule{Use: "a/x", Time: peak}, "claude", 10, false}, // a send-to rule pauses nothing
	} {
		if got := c.r.Pauses(c.agent, "", at(2, c.hour, 0)); got != c.want {
			t.Errorf("%+v %s %d: %v", c.r, c.agent, c.hour, got)
		}
	}
	// saved as "pause": true, and back
	b, _ := json.Marshal(rules[0])
	if !strings.Contains(string(b), `"pause":true`) {
		t.Fatal(string(b))
	}
}

// Typed: pause=<model> with hours, days or agents, read back the same.
func TestParsePauseRule(t *testing.T) {
	g := Group{ID: "g", Members: []string{"zhipu/glm-5.3", "kimi/k3"}}
	r, _, _, err := ParseRule(g, RuleWords("pause=glm-5.3 time=14:00-18:00 days=mon-fri"))
	if err != nil || !r.Pause || r.Use != "zhipu/glm-5.3" || r.Time == nil {
		t.Fatalf("%+v %v", r, err)
	}
	r.Time.clean()
	if got := r.Line(); got != "pause=zhipu/glm-5.3 time=14:00-18:00 days=mon-fri" {
		t.Fatal(got)
	}
	for _, bad := range []string{"pause=glm-5.3", "pause=glm-5.3 tokens=200k time=09:00-10:00", "pause=glm-5.3 images agents=codex"} {
		if _, _, _, err := ParseRule(g, RuleWords(bad)); err == nil {
			t.Errorf("%s parsed", bad)
		}
	}
}

// A pause leaves out the models it holds for: the group's own, every model
// of a group in it it names, or the model a group in the group pauses.
func TestPausedOut(t *testing.T) {
	peak := &TimeWindow{From: "09:00", To: "18:00"}
	sub := Group{ID: "cheap", Members: []string{"c/m", "d/m"}, Rules: []Rule{{Use: "d/m", Pause: true, Time: peak}}}
	ms := []Member{
		{ID: "a/x", Path: []string{"a/x"}, Provider: Provider{ID: "a"}, Model: "x"},
		{ID: "group/cheap", Path: []string{"group/cheap", "c/m"}, Via: []Group{sub}, Provider: Provider{ID: "c"}, Model: "m"},
		{ID: "group/cheap", Path: []string{"group/cheap", "d/m"}, Via: []Group{sub}, Provider: Provider{ID: "d"}, Model: "m"},
		{ID: "b/y", Path: []string{"b/y"}, Provider: Provider{ID: "b"}, Model: "y", Effort: "high"},
	}
	names := func(ms []Member) (out []string) {
		for _, m := range ms {
			out = append(out, m.Provider.ID+"/"+m.Model)
		}
		return out
	}
	// the group's own model, at its effort, and a group in it's own pause
	g := Group{ID: "top", Rules: []Rule{{Use: "b/y", Pause: true, Time: peak}}}
	kept, paused := PausedOut(g, ms, "claude", "", at(2, 10, 0))
	if strings.Join(names(kept), ",") != "a/x,c/m" || len(paused) != 2 || paused[0].Member != "d/m" || paused[0].Group != "cheap" || paused[1].Member != "b/y:high" || paused[1].Group != "" {
		t.Fatalf("%v %+v", names(kept), paused)
	}
	// a group in the group named: all its models
	g.Rules = []Rule{{Use: "group/cheap", Pause: true, Time: peak}}
	if kept, _ = PausedOut(g, ms, "claude", "", at(2, 10, 0)); strings.Join(names(kept), ",") != "a/x,b/y" {
		t.Fatal(names(kept))
	}
	// outside the hours, nobody
	if kept, paused = PausedOut(g, ms, "claude", "", at(2, 19, 0)); len(kept) != 4 || len(paused) != 0 {
		t.Fatal(names(kept), paused)
	}
}
