package gui

import (
	"os"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A provider's "Only through routing groups" switched off brings its
// models back to agents' lists, unless a routing group listed alone
// (Group.Unlists: the group listed alone) keeps them out: then they stayed out
// of every agent's list with the provider's editor saying agents see them
// (hayden4608 on Discord). The editor is told which groups keep which of
// its models out, to say so.
func TestUnlistedGroupKeptSaid(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m2", "m3"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "alone", Name: "Alone", Members: []string{"relay/m2"}, Unlists: true}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "also", Name: "Also", Members: []string{"relay/m3"}}); err != nil {
		t.Fatal(err)
	}
	var listed []string
	for _, e := range provider.Catalog() {
		if e.Group == "" {
			listed = append(listed, e.ID)
		}
	}
	if !slices.Equal(listed, []string{"relay/m3"}) {
		t.Fatalf("agents are offered %v, want relay/m3 alone", listed)
	}
	p, err := provider.Find("relay")
	if err != nil {
		t.Fatal(err)
	}
	j := providerInfo(*p, nil)
	if !slices.Equal(j.KeptBy["m2"], []string{"Alone"}) || len(j.KeptBy) != 1 {
		t.Fatalf("keptBy %v, want m2 kept by Alone alone", j.KeptBy)
	}
}
