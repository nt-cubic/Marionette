package cli

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// routing=weight routes a group by its members' weights (#1910, DD on
// Discord), weight= sets those it names and keeps the rest, the group
// shows each member's, and a weight outside 1 to 1000 is refused.
func TestGroupWeightCLI(t *testing.T) {
	groupsHome(t)
	g, err := addGroup("Spread", []string{"models=a/m:high,b/vendor/m", "routing=weighted-round-robin", "weight=a/m:high=3"})
	if err != nil {
		t.Fatal(err)
	}
	if g.Routing != provider.Weighted || g.MemberWeight("a/m:high") != 3 || g.MemberWeight("b/vendor/m") != 1 {
		t.Fatalf("routing %q weights %v", g.Routing, g.Weights)
	}
	out, err := said(t, func() error { return showGroup(g) })
	if err != nil || !strings.Contains(out, "weight") || !strings.Contains(out, "· weight 3") || !strings.Contains(out, "· weight 1") {
		t.Fatalf("shown %v:\n%s", err, out)
	}
	if g, err = setGroup("spread", []string{"weight=b/vendor/m=5"}); err != nil || g.MemberWeight("a/m:high") != 3 || g.MemberWeight("b/vendor/m") != 5 {
		t.Fatalf("weight=b/vendor/m=5: %v %v", g.Weights, err)
	}
	for _, bad := range []string{"weight=b/vendor/m=0", "weight=b/vendor/m=1001", "weight=b/vendor/m", "weight=c/m=2"} {
		if _, err := setGroup("spread", []string{bad}); err == nil {
			t.Fatalf("%s taken", bad)
		}
	}
	if g, err = setGroup("spread", []string{"weight="}); err != nil || len(g.Weights) != 0 {
		t.Fatalf("weight= %v %v", g.Weights, err)
	}
}
