package cli

import (
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// magpie group pick / pick= route a group manually (#317): every request
// to the model picked, stored with the group, the others left aside.
func TestGroupPick(t *testing.T) {
	groupsHome(t)
	if v, err := parseRouting("manual"); err != nil || v != provider.Manual {
		t.Fatalf("routing manual: %q %v", v, err)
	}
	if _, err := addGroup("Two", []string{"models=a/m,b/vendor/m"}); err != nil {
		t.Fatal(err)
	}
	g, err := setGroup("two", []string{"pick=vendor/m"}) // a bare model id one member has
	if err != nil {
		t.Fatal(err)
	}
	if g.Routing != provider.Manual || g.Pick != "b/vendor/m" || g.Picked() != "b/vendor/m" {
		t.Fatalf("picked: %+v", g)
	}
	if gs := storedGroups(t); len(gs) != 1 || gs[0]["routing"] != "manual" || gs[0]["pick"] != "b/vendor/m" {
		t.Fatalf("stored: %v", gs)
	}
	if _, ms, ok := provider.FindGroup("group/two"); !ok || len(ms) != 1 || ms[0].Provider.ID != "b" {
		t.Fatalf("members routed to: %+v", ms)
	}
	if err := groupCmd([]string{"group", "pick", "two", "a/m"}); err != nil {
		t.Fatal(err)
	}
	if _, ms, _ := provider.FindGroup("group/two"); len(ms) != 1 || ms[0].Provider.ID != "a" {
		t.Fatalf("picked a/m: %+v", ms)
	}
	if _, err := setGroup("two", []string{"pick=only-a"}); err == nil {
		t.Fatal("picked a model not in the group")
	}
	// routed otherwise, every member again; the pick kept for next time
	g, err = setGroup("two", []string{"routing=smart"})
	if err != nil {
		t.Fatal(err)
	}
	if _, ms, _ := provider.FindGroup("group/two"); g.Pick != "a/m" || len(ms) != 2 {
		t.Fatalf("smart again: %+v %d", g, len(ms))
	}
	// a model taken out of the group is no longer its pick
	if g, err = setGroup("two", []string{"routing=manual", "models-=a/m"}); err != nil || g.Picked() != "b/vendor/m" {
		t.Fatalf("pick taken out: %+v %v", g, err)
	}
}
