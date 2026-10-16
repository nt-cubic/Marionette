package provider

import (
	"strings"
	"testing"
)

// A group switched off as a whole (PAMI on Discord) is kept as it is, but
// agents aren't offered it, it routes nowhere, a group that has it in it
// skips it, and a save leaves it off; switched on again it is all back.
func TestSwitchGroup(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{Name: "Inner", Members: []string{"a/x"}}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Outer", Members: []string{"group/inner", "b/x"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	listed := func(id string) bool {
		for _, e := range Catalog() {
			if e.ID == id {
				return true
			}
		}
		return false
	}
	outer := func() string {
		t.Helper()
		_, ms, ok := FindGroup("group/outer")
		if !ok {
			return "-"
		}
		var out []string
		for _, m := range ms {
			out = append(out, m.Provider.ID+"/"+m.Model)
		}
		return strings.Join(out, " ")
	}
	if s := outer(); s != "a/x b/x" || !listed("group/inner") {
		t.Fatalf("on: %s, listed %v", s, listed("group/inner"))
	}

	if err := SwitchGroup("inner", false); err != nil {
		t.Fatal(err)
	}
	if _, _, ok := FindGroup("group/inner"); ok {
		t.Fatal("a group switched off still routes")
	}
	if listed("group/inner") {
		t.Fatal("a group switched off is still offered to agents")
	}
	if s := outer(); s != "b/x" {
		t.Fatalf("the group that has it in it: %s", s)
	}
	if g, ok := DisabledGroup("group/inner"); !ok || g.Name != "Inner" {
		t.Fatalf("DisabledGroup: %v %+v", ok, g)
	}
	if _, ok := DisabledGroup("inner"); !ok {
		t.Fatal("DisabledGroup by the id without group/")
	}
	if _, ok := DisabledGroup("group/outer"); ok {
		t.Fatal("a group on is no switched-off one")
	}
	// kept on the page, as it was
	var inner Group
	for _, g := range Groups() {
		if g.ID == "inner" {
			inner = g
		}
	}
	if !inner.Disabled || len(inner.Members) != 1 {
		t.Fatalf("listed: %+v", inner)
	}
	// a save from the editor (which sends no "disabled") leaves it off
	inner.Disabled = false
	inner.Members = append(inner.Members, "b/x")
	if err := SaveGroup(inner); err != nil {
		t.Fatal(err)
	}
	if _, ok := DisabledGroup("group/inner"); !ok {
		t.Fatal("a save switched it on")
	}
	// it classifies for no one while off
	if err := SaveGroup(Group{Name: "Ruled", Members: []string{"b/x"}, Classifier: "group/inner", Effort: EffortAuto}); err == nil || !strings.Contains(err.Error(), "switched off") {
		t.Fatalf("classifier switched off: %v", err)
	}

	if err := SwitchGroup("inner", true); err != nil {
		t.Fatal(err)
	}
	if s := outer(); s != "a/x b/x" || !listed("group/inner") {
		t.Fatalf("on again: %s, listed %v", s, listed("group/inner"))
	}
	// one classifying for another stays on
	if err := SaveGroup(Group{Name: "Ruled", Members: []string{"b/x"}, Classifier: "group/inner", Effort: EffortAuto}); err != nil {
		t.Fatal(err)
	}
	if err := SwitchGroup("inner", false); err == nil || !strings.Contains(err.Error(), "classifier") {
		t.Fatalf("classifier: %v", err)
	}
	if err := SwitchGroup("nope", false); err == nil {
		t.Fatal("a group magpie hasn't")
	}
}
