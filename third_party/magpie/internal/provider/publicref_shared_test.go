package provider

import (
	"fmt"
	"testing"

	"github.com/yetone/magpie/internal/filememo"
)

// PublicRef reads the groups magpie holds without copying them: it is asked
// for every model a page lists, and each copied every group and its rules
// (115 MB of a live magpie's 2.6 GB in a minute and a half). A group saved
// since is seen, and DisabledGroup's caller still gets a group of its own.
func TestPublicRefCopiesNoGroups(t *testing.T) {
	autoRefHome(t)
	if err := SaveGroup(Group{Name: "Mine", Members: []string{"a/x", "b/x"}}); err != nil {
		t.Fatal(err)
	}
	// the groups a user with many keeps: each with members and a rule
	for i := range 40 {
		if err := SaveGroup(Group{Name: fmt.Sprintf("G%d", i), Members: []string{"a/x", "b/x", "c/x"},
			Rules: []Rule{{Use: "a/x", Agents: []string{"claude"}, Folders: []string{"/w"}}}}); err != nil {
			t.Fatal(err)
		}
	}
	defer Hold()()
	defer filememo.Hold()()
	if got := PublicRef("group/auto-claude-opus-5-5:high[1m]"); got != "auto/claude-opus-5.5:high[1m]" {
		t.Fatalf("the found group's ref: %q", got)
	}
	copied := testing.AllocsPerRun(5, func() { Groups() })
	asked := testing.AllocsPerRun(20, func() { PublicRef("group/auto-claude-opus-5-5") })
	if asked*10 > copied {
		t.Fatalf("PublicRef took %.0f allocations, a copy of the groups %.0f", asked, copied)
	}

	if err := SwitchGroup("mine", false); err != nil {
		t.Fatal(err)
	}
	d, ok := DisabledGroup("group/mine")
	if !ok {
		t.Fatal("a group switched off since is not found off")
	}
	d.Members[0] = "changed"
	if g, _ := groupOf(sharedGroups(), "mine"); g.Members[0] == "changed" {
		t.Fatal("DisabledGroup's caller wrote into the groups held")
	}
}
