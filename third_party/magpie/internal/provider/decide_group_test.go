package provider

import (
	"strings"
	"testing"
)

// A group of decision models alone saves, answers System One and stays out
// of the agents' catalog; one beside chat models, or inside a chat group,
// or with what shapes a conversation, is refused (ARNO on Discord).
func TestDecisionGroup(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, p := range []Provider{
		{ID: "a", Name: "a", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m"}},
		{ID: "ja", Name: "JA", Key: "k1", Decide: "http://127.0.0.1:1/v1"},
		{ID: "jb", Name: "JB", Key: "k2", Decide: "http://127.0.0.1:1/v1"},
	} {
		if err := Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{ID: "jevs", Name: "Jevs", Members: []string{"ja/jev-latest", "jb/jev-latest"}}); err != nil {
		t.Fatal(err)
	}
	g, ms, ok := FindGroup("group/jevs")
	if !ok || !g.Decides() || !DecisionGroup(ms) || len(ms) != 2 {
		t.Fatalf("decision group %+v %+v %v", g, ms, ok)
	}
	if id, ok := GroupFor("Jevs"); !ok || id != "group/jevs" {
		t.Fatalf("by name: %q %v", id, ok)
	}
	for _, e := range Catalog() {
		if e.ID == "group/jevs" || strings.HasPrefix(e.ID, "group/jevs") {
			t.Fatalf("a decision group in the agents' catalog: %+v", e)
		}
	}
	if _, _, ok := Resolve("group/jevs"); ok {
		t.Fatal("a decision group resolves as a chat model")
	}
	for _, tc := range []struct {
		g   Group
		err string
	}{
		{Group{Name: "Mixed", Members: []string{"ja/jev-latest", "a/m"}}, "decision models only"},
		{Group{Name: "Chat", Members: []string{"a/m", "group/jevs"}}, "classifier, not one of its models"},
		{Group{Name: "P", Members: []string{"ja/jev-latest"}, Match: []string{"jev*"}}, "not by a pattern"},
		{Group{Name: "R", Members: []string{"ja/jev-latest"}, Effort: EffortAuto, Classifier: "jb/jev-latest"}, "no conversation"},
		{Group{Name: "E", Members: []string{"ja/jev-latest:high"}}, "no effort of its own"},
	} {
		if err := SaveGroup(tc.g); err == nil || !strings.Contains(err.Error(), tc.err) {
			t.Errorf("%s: %v, want %q", tc.g.Name, err, tc.err)
		}
	}
	// a chat group still takes the decision group as its classifier
	if err := SaveGroup(Group{Name: "C", Members: []string{"a/m"}, Effort: EffortAuto, Classifier: "group/jevs"}); err != nil {
		t.Errorf("decision group as a classifier: %v", err)
	}
}
