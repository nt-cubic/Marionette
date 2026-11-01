package provider

import (
	"slices"
	"testing"
)

// The providers' page asked MemberGroups once for each provider, and each
// built the groups' members over the whole catalog again: 11.6 MB of
// what twenty polls allocated. While a request holds the catalog it is
// built once, and a group saved is in the next request's.
func TestMemberGroupsBuiltOnceWhileHeld(t *testing.T) {
	groupHeldFixture(t)
	look := func() map[string][]string {
		defer Hold()()
		var in map[string][]string
		for range 12 {
			in = MemberGroups()
		}
		return in
	}
	look()
	before := memberGroupsBuilt.Load()
	release := Hold()
	for range 20 {
		if gs := MemberGroups()["p2/m1"]; !slices.Contains(gs, GroupPrefix+"g4") {
			t.Fatalf("p2/m1 is in %q, not group/g4", gs)
		}
	}
	release()
	if n := memberGroupsBuilt.Load() - before; n > 1 {
		t.Fatalf("20 looks under one Hold built the member groups %d times, want 1 at most", n)
	}
	if err := SaveGroup(Group{ID: "g5", Name: "G5", Members: []string{"p5/m5"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	if gs := look()["p5/m5"]; !slices.Contains(gs, GroupPrefix+"g5") {
		t.Fatalf("a group saved: p5/m5 is in %q, not group/g5", gs)
	}
}
