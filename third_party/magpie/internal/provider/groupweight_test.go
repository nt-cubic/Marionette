package provider

import (
	"maps"
	"strings"
	"testing"
)

// A group routed by weight (#1910) keeps each member's weight under the
// member as Members spells it: a member typed :fast, a default of 1 left
// out, one not in the group dropped, and one outside 1 to 1000 refused.
// The weight follows its member when the member's effort, its provider's
// id or a group in it is renamed, and when the group is shared to another
// magpie.
func TestSaveGroupWeights(t *testing.T) {
	fastHome(t)
	weights := func(id string) map[string]int {
		t.Helper()
		g, _, ok := FindGroup(GroupPrefix + id)
		if !ok {
			t.Fatalf("no group %s", id)
		}
		return g.Weights
	}
	if err := SaveGroup(Group{ID: "in", Name: "In", Members: []string{"rl/claude-opus-5-5", "an/claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	g := Group{ID: "spread", Name: "Spread", Routing: Weighted,
		Members: []string{"oa/gpt-6.1-sol:high:fast", "rl/gpt-6.1-sol", "an/claude-opus-5-5", "group/in"},
		Weights: map[string]int{"oa/gpt-6.1-sol:high:fast": 3, "rl/gpt-6.1-sol": 1, "an/claude-opus-5-5": 0, "group/in": 7, "an/claude-sonnet-5": 9}}
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	saved, _, _ := FindGroup(GroupPrefix + "spread")
	if saved.Routing != Weighted {
		t.Fatalf("routing %q", saved.Routing)
	}
	if want := map[string]int{"oa/gpt-6.1-sol:high": 3, "group/in": 7}; !maps.Equal(saved.Weights, want) {
		t.Fatalf("weights %v, want %v", saved.Weights, want)
	}
	if saved.MemberWeight("rl/gpt-6.1-sol") != 1 || saved.MemberWeight("oa/gpt-6.1-sol:high") != 3 {
		t.Fatalf("member weights %d %d", saved.MemberWeight("rl/gpt-6.1-sol"), saved.MemberWeight("oa/gpt-6.1-sol:high"))
	}
	for _, w := range []int{-1, MaxKeyWeight + 1} {
		bad := saved
		bad.Weights = map[string]int{"rl/gpt-6.1-sol": w}
		if err := SaveGroup(bad); err == nil || !strings.Contains(err.Error(), "weight") {
			t.Fatalf("weight %d: %v", w, err)
		}
	}

	// its effort changed: the weight goes with it
	saved.RenameMember("oa/gpt-6.1-sol:high", "oa/gpt-6.1-sol:low")
	if saved.MemberWeight("oa/gpt-6.1-sol:low") != 3 || saved.MemberWeight("oa/gpt-6.1-sol:high") != 1 {
		t.Fatalf("after RenameMember %v", saved.Weights)
	}
	saved.SetMemberWeight("rl/gpt-6.1-sol", 2)
	saved.SetMemberWeight("group/in", 1)
	if err := SaveGroup(saved); err != nil {
		t.Fatal(err)
	}
	if want := map[string]int{"oa/gpt-6.1-sol:low": 3, "rl/gpt-6.1-sol": 2}; !maps.Equal(weights("spread"), want) {
		t.Fatalf("after SetMemberWeight %v, want %v", weights("spread"), want)
	}

	// the provider renamed, a group in it renamed
	if err := Rename("rl", "relay"); err != nil {
		t.Fatal(err)
	}
	saved, _, _ = FindGroup(GroupPrefix + "spread")
	saved.SetMemberWeight("group/in", 4)
	if err := SaveGroup(saved); err != nil {
		t.Fatal(err)
	}
	if err := RenameGroup("in", "inner"); err != nil {
		t.Fatal(err)
	}
	if want := map[string]int{"oa/gpt-6.1-sol:low": 3, "relay/gpt-6.1-sol": 2, "group/inner": 4}; !maps.Equal(weights("spread"), want) {
		t.Fatalf("after the renames %v, want %v", weights("spread"), want)
	}

	// shared to another magpie, under the same models
	text, err := ShareGroup("spread")
	if err != nil {
		t.Fatal(err)
	}
	if err := DeleteGroup("spread"); err != nil {
		t.Fatal(err)
	}
	if _, err := ImportGroup(text); err != nil {
		t.Fatal(err)
	}
	var back Group
	for _, x := range Groups() {
		if x.Name == "Spread" {
			back = x
		}
	}
	if back.Routing != Weighted || back.MemberWeight("relay/gpt-6.1-sol") != 2 || back.MemberWeight("oa/gpt-6.1-sol:low") != 3 {
		t.Fatalf("imported %q %v", back.Routing, back.Weights)
	}
}
