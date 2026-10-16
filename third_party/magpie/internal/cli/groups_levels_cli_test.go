package cli

import (
	"fmt"
	"testing"
)

// levels= names the reasoning levels agents are offered for a group
// (#295): kept lowest first, only those magpie knows; empty is its
// members' shared levels again.
func TestGroupSetLevels(t *testing.T) {
	groupsHome(t)
	if _, err := addGroup("Mix", []string{"models=a/m,b/gpt-5.5", "levels=XHigh,medium low"}); err != nil {
		t.Fatal(err)
	}
	if gs := storedGroups(t); len(gs) != 1 || fmt.Sprint(gs[0]["levels"]) != "[low medium xhigh]" {
		t.Fatalf("stored: %v", gs)
	}
	if _, err := setGroup("mix", []string{"levels=low,turbo"}); err == nil {
		t.Fatal("turbo taken")
	}
	if _, err := setGroup("mix", []string{"levels="}); err != nil {
		t.Fatal(err)
	}
	if gs := storedGroups(t); gs[0]["levels"] != nil {
		t.Fatalf("cleared: %v", gs)
	}
}
