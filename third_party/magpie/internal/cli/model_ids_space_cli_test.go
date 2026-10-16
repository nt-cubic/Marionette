package cli

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A model id can hold a space ("Opus 5.5" on a relay, #1976): models=,
// a group's members and weight= (and fast=) split their lists at commas only,
// so the id stays one model, while a comma list still adds several.
func TestModelIDsKeepTheirSpace(t *testing.T) {
	var p provider.Provider
	if err := applyPairs(&p, []string{"models=Opus 5.5, gpt-5,Claude Opus 5.5"}); err != nil {
		t.Fatal(err)
	}
	if want := []string{"Opus 5.5", "gpt-5", "Claude Opus 5.5"}; !slices.Equal(p.Models, want) {
		t.Fatalf("models = %q, want %q", p.Models, want)
	}

	groupsHome(t)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"Opus 5.5"}, Chat: "http://127.0.0.1:1/v1"}); err != nil {
		t.Fatal(err)
	}
	g, err := addGroup("Spaced", []string{"models=relay/Opus 5.5, b/vendor/m", "routing=weighted-round-robin", "weight=relay/Opus 5.5=3"})
	if err != nil {
		t.Fatal(err)
	}
	if want := []string{"relay/Opus 5.5", "b/vendor/m"}; !slices.Equal(g.Members, want) {
		t.Fatalf("members = %q, want %q", g.Members, want)
	}
	if g.MemberWeight("relay/Opus 5.5") != 3 {
		t.Fatalf("weights %v", g.Weights)
	}
	if g, err = setGroup("spaced", []string{"models-=relay/Opus 5.5"}); err != nil || !slices.Equal(g.Members, []string{"b/vendor/m"}) {
		t.Fatalf("models-= left %q: %v", g.Members, err)
	}
}
