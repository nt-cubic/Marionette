package provider

import (
	"slices"
	"strings"
	"testing"
)

// A group that names its reasoning levels (#295) offers those, whatever
// its members have in common; one that doesn't offers what they share,
// as before. A group in a group offers its own for its models. Only the
// levels magpie knows are taken, kept lowest first.
func TestGroupLevelsOverride(t *testing.T) {
	effortHome(t)
	if err := SetModelEfforts("a/m", []string{"low", "high", "max"}); err != nil { // deepseek-flash
		t.Fatal(err)
	}
	if err := SetModelEfforts("a/n", []string{"none", "low", "medium", "high", "xhigh", "max"}); err != nil { // gpt-6-sol
		t.Fatal(err)
	}
	entry := func(id string) Entry {
		t.Helper()
		for _, e := range Catalog() {
			if e.ID == GroupPrefix+id {
				return e
			}
		}
		t.Fatalf("%s not listed", id)
		return Entry{}
	}
	if err := SaveGroup(Group{Name: "mix", Members: []string{"a/n", "a/m"}}); err != nil {
		t.Fatal(err)
	}
	if e := entry("mix"); !slices.Equal(e.Efforts, []string{"low", "high", "max"}) || !slices.Equal(e.Shared, e.Efforts) {
		t.Fatalf("unset: levels %v, shared %v", e.Efforts, e.Shared)
	}
	g, _, _ := FindGroup("group/mix")
	g.Levels = []string{"MAX", " xhigh", "medium", "low", "high", "none", "medium"}
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	all := []string{"none", "low", "medium", "high", "xhigh", "max"}
	if g, _, _ := FindGroup("group/mix"); !slices.Equal(g.Levels, all) {
		t.Fatalf("kept %v", g.Levels)
	}
	if e := entry("mix"); !slices.Equal(e.Efforts, all) || !slices.Equal(e.Shared, []string{"low", "high", "max"}) {
		t.Fatalf("named: levels %v, shared %v", e.Efforts, e.Shared)
	}
	// a group with mix in it offers mix's levels for mix's models
	if err := SaveGroup(Group{Name: "outer", Members: []string{"group/mix", "a/n"}}); err != nil {
		t.Fatal(err)
	}
	if e := entry("outer"); !slices.Equal(e.Efforts, all) {
		t.Fatalf("outer: levels %v", e.Efforts)
	}
	// a level magpie doesn't know is refused, the group left as it was
	g.Levels = []string{"low", "turbo"}
	if err := SaveGroup(g); err == nil || !strings.Contains(err.Error(), "turbo") {
		t.Fatalf("turbo: %v", err)
	}
	// cleared, it offers what its members share again
	g.Levels = nil
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	if e := entry("mix"); !slices.Equal(e.Efforts, []string{"low", "high", "max"}) {
		t.Fatalf("cleared: levels %v", e.Efforts)
	}
	if e := entry("outer"); !slices.Equal(e.Efforts, []string{"low", "high", "max"}) {
		t.Fatalf("outer, cleared: levels %v", e.Efforts)
	}
}
