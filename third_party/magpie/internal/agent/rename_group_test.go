package agent

import (
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
)

// A routing group given another id takes the agents on it along, as a
// provider's rename does: Codex, Claude Code (its model and tiers),
// OpenCode and Pi were left on group/<old>, a group magpie no longer had,
// and the user had to pick it again in each (lucis0116 on X).
func TestRenameGroupMovesTheAgentsOnIt(t *testing.T) {
	home := syncHome(t)
	if err := provider.SaveGroup(provider.Group{ID: "smart", Name: "Smart", Members: []string{"relay/glm-4.6"}}); err != nil {
		t.Fatal(err)
	}
	codexCfg := filepath.Join(home, ".codex", "config.toml")
	codexCat := filepath.Join(home, ".codex", "magpie-models.json")
	writeFile(t, codexCfg, "model = \"group/smart\"\nmodel_provider = \"magpie\"\nmodel_catalog_json = '"+codexCat+"'\n")
	writeFile(t, codexCat, `{"models":[]}`)
	piModels := filepath.Join(home, ".pi", "agent", "models.json")
	piSettings := filepath.Join(home, ".pi", "agent", "settings.json")
	writeFile(t, piModels, `{"providers":{"magpie":{"name":"magpie","models":[]}}}`)
	writeFile(t, piSettings, `{"defaultProvider":"magpie","defaultModel":"group/smart"}`)
	oc := filepath.Join(home, ".config", "opencode", "opencode.json")
	writeFile(t, oc, `{"model":"magpie/group/smart"}`)
	claudeCfg := filepath.Join(home, ".claude", "settings.json")
	writeFile(t, claudeCfg, `{}`)
	cc, err := Find("claude")
	if err != nil {
		t.Fatal(err)
	}
	if err := cc.Apply("model", "group/smart"); err != nil {
		t.Fatal(err)
	}
	SyncCatalog()

	moved, err := RenameGroup("smart", "clever")
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"Codex", "Claude Code", "OpenCode", "Pi"} {
		if !slices.Contains(moved, name) {
			t.Errorf("%s not said moved: %v", name, moved)
		}
	}
	if v, _ := edit.GetTOMLTop(codexCfg, "model"); v != "group/clever" {
		t.Errorf("codex model = %q", v)
	}
	if v, _ := edit.GetJSON(piSettings, "defaultModel"); v != "group/clever" {
		t.Errorf("pi defaultModel = %v", v)
	}
	if v, _ := edit.GetJSON(oc, "model"); v != "magpie/group/clever" {
		t.Errorf("opencode model = %v", v)
	}
	claude := readFile(claudeCfg)
	if strings.Contains(claude, "group/smart") || !strings.Contains(claude, `"model": "group/clever"`) && !strings.Contains(claude, `"model":"group/clever"`) {
		t.Errorf("claude settings still on the old id:\n%s", claude)
	}
	for _, f := range []string{codexCat, piModels, oc} {
		if s := readFile(f); strings.Contains(s, "group/smart") || !strings.Contains(s, "group/clever") {
			t.Errorf("%s lists the old id:\n%s", f, s)
		}
	}
}

// The Codex app's app-server reads its model catalog only as it starts,
// so the route it is restarted on (codex_app_server.go) follows what the
// catalog holds: a provider's name changed in magpie reached the file, and
// the app kept showing the old one (lucis0116 on X). The same bytes
// written again are no change.
func TestCodexAppRouteFollowsTheCatalog(t *testing.T) {
	home := syncHome(t)
	codexCat := filepath.Join(home, ".codex", "magpie-models.json")
	writeFile(t, filepath.Join(home, ".codex", "config.toml"), "model = \"relay/glm-4.6\"\nmodel_provider = \"magpie\"\nmodel_catalog_json = '"+codexCat+"'\n")
	writeFile(t, codexCat, `{"models":[]}`)
	SyncCatalog()
	route := func() string {
		t.Helper()
		r, _, known := codexAppRoute()
		if !known {
			t.Fatal("route not known")
		}
		return r
	}
	before := route()
	if !strings.Contains(readFile(codexCat), "GLM-4.6 · Relay") {
		t.Fatalf("catalog:\n%s", readFile(codexCat))
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Renamed", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"glm-4.6"}}); err != nil {
		t.Fatal(err)
	}
	SyncCatalog()
	if !strings.Contains(readFile(codexCat), "GLM-4.6 · Renamed") {
		t.Fatalf("catalog not rewritten:\n%s", readFile(codexCat))
	}
	renamed := route()
	if renamed == before {
		t.Fatal("a provider renamed left the Codex app's route as it was: its app-server keeps the old names")
	}
	// written again as it is (a Connect does): no restart for that
	if err := writeOwnList(codexCat, []byte(readFile(codexCat))); err != nil {
		t.Fatal(err)
	}
	if route() != renamed {
		t.Error("the same catalog written again changed the route")
	}
}
