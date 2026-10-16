package provider

import (
	"slices"
	"strings"
	"testing"
)

// A member switched off (#feedback on Discord: testing routing rules meant
// taking models out and adding them back, losing their order) keeps its
// place and its rules but is sent nothing until switched on again.
func TestGroupMemberOff(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, id := range []string{"a", "b", "c"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
			t.Fatal(err)
		}
	}
	ids := func() string {
		t.Helper()
		_, ms, ok := FindGroup("group/g")
		if !ok {
			return "-"
		}
		var out []string
		for _, m := range ms {
			out = append(out, m.ID)
		}
		return strings.Join(out, " ")
	}
	// one not in the group, and one named twice, are dropped
	if err := SaveGroup(Group{Name: "G", Members: []string{"a/x", "b/x", "c/x"}, Off: []string{"a/x", "z/x", "a/x"}, Routing: Ordered,
		Rules: []Rule{{Use: "a/x", Tokens: 10}}}); err != nil {
		t.Fatal(err)
	}
	g, _, _ := FindGroup("group/g")
	if !slices.Equal(g.Members, []string{"a/x", "b/x", "c/x"}) || !slices.Equal(g.Off, []string{"a/x"}) || len(g.Rules) != 1 {
		t.Fatalf("saved: %+v", g)
	}
	if s := ids(); s != "b/x c/x" {
		t.Fatalf("routed: %s", s)
	}
	if p, _, ok := Resolve("group/g"); !ok || p.ID != "b" {
		t.Fatalf("answers for it: %v %s", ok, p.ID)
	}
	// its effort changed, it stays off
	g.RenameMember("a/x", "a/x:high")
	if !slices.Equal(g.Off, []string{"a/x:high"}) {
		t.Fatalf("renamed: %v", g.Off)
	}
	// every one off leaves the group nothing to send to
	g.Off = g.Members
	if err := SaveGroup(g); err == nil || !strings.Contains(err.Error(), "switched off") {
		t.Fatalf("all off: %v", err)
	}
	// a manual group sends to its pick, whatever is off
	g.Routing, g.Pick = Manual, "c/x"
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	if s := ids(); s != "c/x" {
		t.Fatalf("manual: %s", s)
	}
	// switched on again: back where it was
	g.Routing, g.Off = Ordered, nil
	if err := SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	if s := ids(); s != "a/x:high b/x c/x" {
		t.Fatalf("on: %s", s)
	}
}
