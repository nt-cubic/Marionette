package provider

import (
	"testing"
)

// A group's own id keeps the dots of a model's name (#968): saved from its
// name or given, renamed to one, and an agent sending the bare name reaches
// it. The ids magpie finds keep Slug's dashes, so none saved before change.
func TestGroupIDKeepsDots(t *testing.T) {
	for in, want := range map[string]string{
		"gpt-6.1-sol":   "gpt-6.1-sol",
		"GPT 6.1 Sol":   "gpt-6.1-sol",
		"a..b":          "a.b",
		".x.":           "x",
		"-.a.-":         "a",
		"claude/opus 5": "claude-opus-5",
		"...":           "",
	} {
		if got := GroupSlug(in); got != want {
			t.Errorf("GroupSlug(%q) = %q, want %q", in, got, want)
		}
	}
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if err := Save(Provider{ID: "a", Name: "a", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-6.1-sol", "x"}}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "GPT 6.1 Sol", Members: []string{"a/gpt-6.1-sol"}}); err != nil {
		t.Fatal(err)
	}
	if _, _, ok := FindGroup("group/gpt-6.1-sol"); !ok {
		t.Fatal("no group/gpt-6.1-sol")
	}
	if g, ok := GroupFor("gpt-6.1-sol"); !ok || g != "group/gpt-6.1-sol" {
		t.Fatalf("GroupFor(gpt-6.1-sol) = %q %v", g, ok)
	}
	if err := SaveGroup(Group{ID: "x..y", Members: []string{"a/x"}}); err == nil {
		t.Fatal("saved x..y")
	}
	if err := SaveGroup(Group{ID: "x.y", Members: []string{"a/x"}}); err != nil {
		t.Fatal(err)
	}
	if err := RenameGroup("x.y", "x.z"); err != nil {
		t.Fatal(err)
	}
	if err := RenameGroup("x.z", ".x"); err == nil {
		t.Fatal("renamed to .x")
	}
	if _, _, ok := FindGroup("group/x.z"); !ok {
		t.Fatal("no group/x.z")
	}
	if id := AutoGroupID("gpt-6.1-sol"); id != "auto-gpt-6-1-sol" {
		t.Fatalf("a found group's id changed: %s", id)
	}
}
