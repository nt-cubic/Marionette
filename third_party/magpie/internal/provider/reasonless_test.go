package provider

import (
	"slices"
	"testing"
)

// Ticking no level of a model whose levels are known says it takes no
// reasoning field at all (yuexian7: a model models.dev lists as reasoning
// that answers any reasoning parameter with 422). Ticking levels again, or
// Restore default, takes that back; the two settings never both hold.
func TestModelReasonless(t *testing.T) {
	effortHome(t)
	if err := SetModelEfforts("a/m", []string{"high"}); err != nil {
		t.Fatal(err)
	}
	if err := SetModelReasonless("a/m", true); err != nil {
		t.Fatal(err)
	}
	if !Reasonless("a", "m") || Reasonless("a", "n") {
		t.Fatal("reasonless not kept for a/m alone")
	}
	if got := (Provider{ID: "a"}).ModelEfforts()["m"]; got != nil {
		t.Fatalf("its kept levels stayed: %v", got)
	}
	if err := SetModelEfforts("a/m", []string{"low", "high"}); err != nil {
		t.Fatal(err)
	}
	if Reasonless("a", "m") {
		t.Fatal("ticking levels left it reasonless")
	}
	if err := SetModelReasonless("a/m", true); err != nil {
		t.Fatal(err)
	}
	if err := SetModelEfforts("a/m", nil); err != nil { // Restore default
		t.Fatal(err)
	}
	if Reasonless("a", "m") {
		t.Fatal("Restore default left it reasonless")
	}
}

// A reasonless member is sent no reasoning, so it doesn't take the group's
// levels away (as a member whose levels nothing knows doesn't, #597), and
// the group is quiet to it alone.
func TestGroupReasonlessMemberKeepsLevels(t *testing.T) {
	groupReasoningHome(t)
	// as entryFor leaves it: no levels, no reasoning, on a provider whose
	// list is known (not #597's unknown, which keeps the levels anyway)
	off := quietMember()
	off.Reasonless = true
	if !off.Quiet() {
		t.Fatal("a reasonless member isn't quiet")
	}
	for name, in := range map[string][]Entry{
		"off last":  {thinksMember(), off},
		"off first": {off, thinksMember()},
	} {
		g := groupEntries(in)
		if len(g) != 1 {
			t.Fatalf("%s: groups %+v", name, g)
		}
		if !g[0].Reasoning || !slices.Equal(g[0].Efforts, levels) {
			t.Errorf("%s: reasoning %v, levels %v, want %v", name, g[0].Reasoning, g[0].Efforts, levels)
		}
	}
}
