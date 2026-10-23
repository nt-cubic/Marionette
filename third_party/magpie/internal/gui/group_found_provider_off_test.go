package gui

import (
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #2671 (timonwong): a provider switched off for a while leaves the found
// group of a model another provider serves on the Routing page, with its
// member said as its provider being off, rather than the group vanishing.
func TestFoundGroupMemberOfASwitchedOffProvider(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := provider.Save(provider.Provider{ID: id, Name: strings.ToUpper(id) + " Cloud", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SetOff("b", true); err != nil {
		t.Fatal(err)
	}
	var found *groupJSON
	st := groupsState()
	for i := range st.Groups {
		if st.Groups[i].ID == "auto-x" {
			found = &st.Groups[i]
		}
	}
	if found == nil {
		t.Fatalf("no group auto-x with b off: %+v", st.Groups)
	}
	info := map[string]memberJSON{}
	for _, m := range found.Info {
		info[m.ID] = m
	}
	if m := info["a/x"]; !m.Ready || m.ProviderOff {
		t.Fatalf("a/x: %+v", m)
	}
	if m := info["b/x"]; m.Ready || !m.ProviderOff || m.Name != "B Cloud" {
		t.Fatalf("b/x, its provider off: %+v", m)
	}
}
