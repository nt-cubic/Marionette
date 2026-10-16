package provider

import (
	"slices"
	"testing"
)

// A group can take another id: a found one drops its auto- prefix and
// stays removed under the old id, and the groups that have it in them
// name it by the new one.
func TestRenameGroup(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-6-astra", "x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{Name: "Top", Members: []string{"a/x", "group/auto-gpt-6-astra"},
		Rules: []Rule{{Use: "group/auto-gpt-6-astra", Tokens: 10}}}); err != nil {
		t.Fatal(err)
	}
	if err := RenameGroup("auto-gpt-6-astra", "top"); err == nil {
		t.Fatal("renamed onto another group")
	}
	if err := RenameGroup("auto-gpt-6-astra", "Not A Slug"); err == nil {
		t.Fatal("renamed to a bad id")
	}
	if err := RenameGroup("auto-gpt-6-astra", "gpt-6-astra"); err != nil {
		t.Fatal(err)
	}
	if _, _, ok := FindGroup("group/auto-gpt-6-astra"); ok {
		t.Fatal("the found group came back beside the renamed one")
	}
	g, ms, ok := FindGroup("group/gpt-6-astra")
	if !ok || g.Auto || len(ms) != 2 {
		t.Fatalf("%v %+v %d", ok, g, len(ms))
	}
	top, _, _ := FindGroup("group/top")
	if !slices.Contains(top.Members, "group/gpt-6-astra") || top.Rules[0].Use != "group/gpt-6-astra" {
		t.Fatalf("%+v", top)
	}
	if !slices.Contains(RemovedGroups(), "auto-gpt-6-astra") {
		t.Fatal(RemovedGroups())
	}
	// a user's group renamed leaves nothing behind
	if err := RenameGroup("top", "best"); err != nil {
		t.Fatal(err)
	}
	if _, _, ok := FindGroup("group/top"); ok || slices.Contains(RemovedGroups(), "top") {
		t.Fatal("top is still there")
	}
	if _, _, ok := FindGroup("group/best"); !ok {
		t.Fatal("no best")
	}
	if err := RenameGroup("nothing", "else"); err == nil {
		t.Fatal("renamed a group that isn't")
	}
}
