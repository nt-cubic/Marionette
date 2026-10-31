package tui

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A group magpie found keeps a member whose provider is switched off
// (#2671), but its line on the routing page lists only those the gateway
// can send to, and says how many are off (XYenon on Discord: a provider
// switched off was still listed in the found group). A group of the user's
// lists what they put in; the group's own page marks the member off.
func TestFoundGroupLineLeavesASwitchedOffProviderOut(t *testing.T) {
	home(t)
	for _, p := range []provider.Provider{
		{ID: "pa", Name: "PA", Key: "k", Models: []string{"luna"}, Chat: "http://127.0.0.1:1/v1"},
		{ID: "pb", Name: "PB", Key: "k", Models: []string{"luna"}, Chat: "http://127.0.0.1:1/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "mine", Name: "Mine", Members: []string{"pa/luna", "pb/luna"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetOff("pb", true); err != nil {
		t.Fatal(err)
	}
	m := press(t, model{w: 160, h: 40}, "3")
	line := func(name string) string {
		t.Helper()
		for _, l := range strings.Split(m.View(), "\n") {
			if strings.Contains(l, name) {
				return l
			}
		}
		t.Fatalf("no %s line:\n%s", name, m.View())
		return ""
	}
	if l := line("auto/luna"); strings.Contains(l, "pb/luna") || !strings.Contains(l, "pa/luna") || !strings.Contains(l, "1 model ") || !strings.Contains(l, "1 switched off") {
		t.Fatalf("found group's line: %q", l)
	}
	if l := line("group/mine"); !strings.Contains(l, "pa/luna → pb/luna") || strings.Contains(l, "switched off") {
		t.Fatalf("the user's group's line: %q", l)
	}
	m.gid, m.mode = "auto-luna", modeGroup
	v := m.View()
	if !strings.Contains(v, "pb/luna · switched off") || strings.Contains(v, "pa/luna · switched off") {
		t.Fatalf("the found group's page:\n%s", v)
	}
	// on again: listed as before
	if err := provider.SetOff("pb", false); err != nil {
		t.Fatal(err)
	}
	m = press(t, model{w: 160, h: 40}, "3")
	if l := line("auto/luna"); !strings.Contains(l, "pa/luna → pb/luna") || strings.Contains(l, "switched off") {
		t.Fatalf("pb on again: %q", l)
	}
}
