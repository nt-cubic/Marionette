package cli

import (
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// magpie group copy <id> [name] duplicates a group (lc on Discord): its
// models and routing under "<name> copy", or the name given, and says what
// agents pick it as. A found group copies too, as one of the user's; a
// removed one is brought back first.
func TestGroupCopyCLI(t *testing.T) {
	groupsHome(t)
	if err := provider.SaveGroup(provider.Group{ID: "fast", Name: "Fast", Members: []string{"a/m", "a/only-a"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	out, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "copy", "fast"}) })
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "Fast copy") || !strings.Contains(out, "group/fast-copy") {
		t.Fatalf("copy said %q", out)
	}
	c, err := findGroup("fast-copy")
	if err != nil || c.Name != "Fast copy" || c.Routing != provider.Ordered || !slices.Equal(c.Members, []string{"a/m", "a/only-a"}) {
		t.Fatalf("%v %+v", err, c)
	}

	if _, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "duplicate", "Fast", "Fast", "B"}) }); err != nil {
		t.Fatal(err)
	}
	if c, err := findGroup("fast-b"); err != nil || c.Name != "Fast B" {
		t.Fatalf("named copy: %v %+v", err, c)
	}

	if _, err := stdoutOf(t, func() error { return groupCmd([]string{"group", "copy", "auto-m", "Mine"}) }); err != nil {
		t.Fatal(err)
	}
	if c, err := findGroup("mine"); err != nil || c.Auto || len(c.Members) != 2 {
		t.Fatalf("found group's copy: %v %+v", err, c)
	}

	if err := groupCmd([]string{"group", "copy", "nope"}); err == nil {
		t.Error("copied a group that isn't")
	}
	if err := groupCmd([]string{"group", "copy"}); err == nil || !strings.Contains(err.Error(), "magpie group copy <id>") {
		t.Errorf("no id: %v", err)
	}
}
