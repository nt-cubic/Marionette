package gateway

// What the classifier said, as it said it (#4598): a group's trace told
// which intent it named, and none when it named no rule's — but not the
// answer itself, so "said none" was the same whether the classifier took
// its own none-of-the-kinds option, named a kind it wasn't sure enough
// of, or answered a number no rule could take. These tests read the
// said of the trace's classified, as the page's JSON has it.

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// saidOf is the said of a classified, as the trace's JSON spells it: the
// field read here by what the field is called, so a change that drops it
// fails these tests rather than the build.
func saidOf(c *Classified) string {
	b, err := json.Marshal(c)
	if err != nil {
		return ""
	}
	var m map[string]any
	if json.Unmarshal(b, &m) != nil {
		return ""
	}
	s, _ := m["said"].(string)
	return s
}

// A group's seventh rule, an intent of its own for everything the first
// six are not ("不属于以上任意一项"): the classifier answering 7 sends the
// turn by it — rule 7 matches, and the trace says it did (#4598).
func TestClassifierChoosingTheFallbackKindMatchesItsRule(t *testing.T) {
	s, _, _, c := intented(t,
		provider.Rule{Use: "a/small", Intent: "a quick question"},
		provider.Rule{Use: "b/big", Intent: "writing or fixing tests"},
		provider.Rule{Use: "a/small", Intent: "refactoring"},
		provider.Rule{Use: "b/big", Intent: "debugging a crash"},
		provider.Rule{Use: "a/small", Intent: "writing docs"},
		provider.Rule{Use: "b/big", Intent: "a long migration"},
		provider.Rule{Use: "b/big", Intent: "不属于以上任意一项"},
	)
	c.set("7", 0)
	out, r := postOK(t, s, "s1", chat("hello there", nil, 0, ""))
	if !strings.Contains(out, "from kb") || r.Rule.N != 7 || r.Rule.Use != "b/big" {
		t.Fatalf("turn 1: %s %+v", out, r.Rule)
	}
	if r.Rule.Classified.Intent != "不属于以上任意一项" {
		t.Fatalf("classified %+v", r.Rule.Classified)
	}
}

// A chat classifier's answer is kept as the number it said: the 0 that
// matches no rule as much as the 2 that matched one (#4598).
func TestClassifiedSaysTheClassifiersAnswer(t *testing.T) {
	s, _, _, c := intented(t,
		provider.Rule{Use: "a/small", Intent: "a quick question"},
		provider.Rule{Use: "b/big", Intent: "writing or fixing tests"},
	)
	c.set("0", 0)
	_, r := postOK(t, s, "s1", chat("hello there", nil, 0, ""))
	if r.Rule.N != 0 || r.Rule.Classified.Intent != "" {
		t.Fatalf("said none: %+v", r.Rule.Classified)
	}
	if said := saidOf(r.Rule.Classified); said != "0" {
		t.Fatalf("the classifier said 0, the trace says %q", said)
	}
	c.set("2", 0)
	_, r = postOK(t, s, "s2", chat("fix the failing test please", nil, 0, ""))
	if r.Rule.N != 2 || r.Rule.Classified.Intent != "writing or fixing tests" {
		t.Fatalf("said 2: %+v", r.Rule.Classified)
	}
	if said := saidOf(r.Rule.Classified); said != "2" {
		t.Fatalf("the classifier said 2, the trace says %q", said)
	}
}

// Jev's choice is kept as it said it, too: "none of these" when it took
// its own none-of-the-kinds option rather than the group's fallback kind,
// and a kind it named without being sure enough of it — which the trace
// said nothing of before, so the two were one and the same "none" (#4598).
func TestClassifiedSaysWhatJevChose(t *testing.T) {
	s, _, _, j := jevved(t, "",
		provider.Rule{Use: "a/small", Intent: "a quick question"},
		provider.Rule{Use: "b/big", Intent: "不属于以上任意一项"},
	)
	j.choice, j.sure = noIntent, 0.9
	_, r := postOK(t, s, "s1", chat("hello there", nil, 0, ""))
	if r.Rule.N != 0 || r.Rule.Classified.Intent != "" {
		t.Fatalf("none of these: %+v", r.Rule.Classified)
	}
	if said := saidOf(r.Rule.Classified); said != noIntent {
		t.Fatalf("Jev chose %q, the trace says %q", noIntent, said)
	}
	j.choice, j.sure = "不属于以上任意一项", 0.2
	_, r = postOK(t, s, "s2", chat("something else entirely", nil, 0, ""))
	if r.Rule.N != 0 || r.Rule.Classified.Intent != "" {
		t.Fatalf("unsure: %+v", r.Rule.Classified)
	}
	if said := saidOf(r.Rule.Classified); said != "不属于以上任意一项" {
		t.Fatalf("Jev chose the fallback kind unsure, the trace says %q", said)
	}
}
