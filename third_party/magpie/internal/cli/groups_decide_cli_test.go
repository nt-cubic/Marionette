package cli

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// magpie group add takes decision models for a group of them alone, asked
// at /v1/systemone (ARNO on Discord), and refuses one beside a chat model.
func TestGroupAddDecisionModels(t *testing.T) {
	groupsHome(t)
	for _, p := range []provider.Provider{
		{ID: "ja", Name: "JA", Key: "k1", Decide: "http://127.0.0.1:1/v1"},
		{ID: "jb", Name: "JB", Key: "k2", Decide: "http://127.0.0.1:1/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	g, err := addGroup("Jevs", []string{"models=ja/jev-latest,jb/jev-latest", "routing=order"})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Join(g.Members, " ") != "ja/jev-latest jb/jev-latest" || !g.Decides() {
		t.Fatalf("%+v", g)
	}
	if _, err := addGroup("Mixed", []string{"models=ja/jev-latest,a/m"}); err == nil || !strings.Contains(err.Error(), "decision models only") {
		t.Fatalf("mixed: %v", err)
	}
}
