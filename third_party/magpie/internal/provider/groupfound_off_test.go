package provider

import (
	"slices"
	"testing"
)

// A group magpie found stays while a provider of it is switched off for a
// while: agents set to it, rules and groups naming it keep working on the
// providers still on, and the one switched off is back in its place once
// it is on again. Only removing a provider takes it out for good.
func TestFoundGroupStaysWithAProviderOff(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	// c serves y alone, so no group is found for it
	for _, id := range []string{"a", "b", "c"} {
		models := []string{"x"}
		if id == "c" {
			models = append(models, "y")
		}
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: models}); err != nil {
			t.Fatal(err)
		}
	}
	members := func() []string {
		t.Helper()
		g, ok := groupOf(Groups(), "auto-x")
		if !ok {
			t.Fatal("no group auto-x")
		}
		if !g.Auto {
			t.Fatalf("auto-x is no longer a found group: %+v", g)
		}
		return g.Members
	}
	routed := func() (out []string) {
		_, ms, _ := FindGroup(GroupPrefix + "auto-x")
		for _, m := range ms {
			out = append(out, m.Provider.ID+"/"+m.Model)
		}
		return out
	}
	listed := func() bool {
		return slices.ContainsFunc(Served(), func(e Entry) bool { return e.ID == AutoPrefix+"x" })
	}
	all := []string{"a/x", "b/x", "c/x"}
	if got := members(); !slices.Equal(got, all) {
		t.Fatalf("found: %v", got)
	}

	// two of three off: one member left, the group stays, in its order
	for _, id := range []string{"a", "b"} {
		if err := SetOff(id, true); err != nil {
			t.Fatal(err)
		}
	}
	if got := members(); !slices.Equal(got, all) {
		t.Fatalf("a and b off, members: %v", got)
	}
	if got := routed(); !slices.Equal(got, []string{"c/x"}) {
		t.Fatalf("a and b off, routes to %v", got)
	}
	if !listed() {
		t.Fatal("a and b off, the group is gone from the models agents are offered")
	}
	if gid, ok := GroupFor("x"); !ok || gid != GroupPrefix+"auto-x" {
		t.Fatalf("a and b off, x names %q %v", gid, ok)
	}
	// a model only one provider has, switched off or not, is no group
	if _, ok := groupOf(Groups(), "auto-y"); ok {
		t.Fatal("y, which only c serves, is a group")
	}

	// every one off: still the group, with nothing to route to
	if err := SetOff("c", true); err != nil {
		t.Fatal(err)
	}
	if got := members(); !slices.Equal(got, all) {
		t.Fatalf("all off, members: %v", got)
	}
	if got := routed(); len(got) != 0 {
		t.Fatalf("all off, routes to %v", got)
	}

	// on again, each is back in its place
	for _, id := range []string{"a", "b", "c"} {
		if err := SetOff(id, false); err != nil {
			t.Fatal(err)
		}
	}
	if got := routed(); !slices.Equal(got, all) {
		t.Fatalf("on again, routes to %v", got)
	}

	// removed, a provider leaves it; with one left there is no group
	for _, id := range []string{"a", "b"} {
		if err := Delete(id); err != nil {
			t.Fatal(err)
		}
	}
	if _, ok := groupOf(Groups(), "auto-x"); ok {
		t.Fatal("a and b removed, auto-x is still found")
	}
}

// A found group the user removed stays removed while a provider of it is
// switched off, and removing it then hides it rather than failing.
func TestFoundGroupRemovedWithAProviderOff(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SetOff("b", true); err != nil {
		t.Fatal(err)
	}
	if err := DeleteGroup("auto-x"); err != nil {
		t.Fatal(err)
	}
	hidden := func() bool {
		i := slices.IndexFunc(Groups(), func(g Group) bool { return g.ID == "auto-x" })
		return i >= 0 && Groups()[i].Hidden
	}
	if !hidden() {
		t.Fatal("removed with b off, auto-x isn't marked removed")
	}
	if err := SetOff("b", false); err != nil {
		t.Fatal(err)
	}
	if !hidden() {
		t.Fatal("b on again, the removed group is back")
	}
}
