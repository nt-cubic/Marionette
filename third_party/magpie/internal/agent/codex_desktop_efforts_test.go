package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
)

// #310: the Codex app's model picker offers only the efforts in its
// [desktop] enabled-reasoning-efforts (unset: low … xhigh, ultra,
// persistent), so a magpie model's "max" was never offered. Routed to one,
// magpie adds what its models take and the app knows; the user's entries,
// order and comments stay, and nothing is written when nothing is missing.
func TestCodexDesktopEfforts(t *testing.T) {
	setup := func(t *testing.T, config string) (string, func() string) {
		home, read := codexHome(t, "", config)
		os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
		os.WriteFile(catalog.CachePath(), []byte(`{
		  "zai":{"models":{"glm-5.3-flash":{"id":"glm-5.3-flash","reasoning_options":[{"type":"effort","values":["low","high","max","turbo"]}]}}}}`), 0o644)
		catalog.Reset()
		t.Cleanup(catalog.Reset)
		if err := provider.Save(provider.Provider{ID: "volc", Name: "Volc", Key: "k", Chat: "http://127.0.0.1:1/api/v3",
			Models: []string{"glm-5.3-flash"}}); err != nil {
			t.Fatal(err)
		}
		return home, read
	}

	t.Run("unset", func(t *testing.T) {
		home, read := setup(t, "# mine\n")
		if err := codex(home).Fields[0].Set("volc/glm-5.3-flash"); err != nil {
			t.Fatal(err)
		}
		cfg := read()
		if !strings.Contains(cfg, "[desktop]\nenabled-reasoning-efforts = [\"low\", \"medium\", \"high\", \"xhigh\", \"ultra\", \"persistent\", \"max\"]\n") ||
			!strings.Contains(cfg, "# mine") || strings.Contains(cfg, "turbo") {
			t.Fatalf("config:\n%s", cfg)
		}
	})

	t.Run("user's own", func(t *testing.T) {
		home, read := setup(t, "[desktop]\n# the ones I use\nenabled-reasoning-efforts = [\"high\", \"low\"] # mine\nother = 1\n")
		if err := codex(home).Fields[0].Set("volc/glm-5.3-flash"); err != nil {
			t.Fatal(err)
		}
		cfg := read()
		if !strings.Contains(cfg, "# the ones I use\nenabled-reasoning-efforts = [\"high\", \"low\", \"max\"] # mine\nother = 1\n") {
			t.Fatalf("config:\n%s", cfg)
		}
		// again: nothing missing, nothing written
		before, _ := os.Stat(filepath.Join(home, ".codex", "config.toml"))
		if err := codexEnableEfforts(filepath.Join(home, ".codex", "config.toml"), magpieModels("codex")); err != nil {
			t.Fatal(err)
		}
		if after, _ := os.Stat(filepath.Join(home, ".codex", "config.toml")); !after.ModTime().Equal(before.ModTime()) || read() != cfg {
			t.Fatalf("rewritten:\n%s", read())
		}
	})

	t.Run("not routed", func(t *testing.T) {
		home, read := setup(t, "")
		if err := codex(home).Fields[0].Set("gpt-5.5"); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); strings.Contains(cfg, "desktop") {
			t.Fatalf("config:\n%s", cfg)
		}
	})

	t.Run("inline desktop", func(t *testing.T) {
		home, read := setup(t, "desktop = { theme = \"dark\" }\n")
		if err := codex(home).Fields[0].Set("volc/glm-5.3-flash"); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); strings.Contains(cfg, "[desktop]") || !strings.Contains(cfg, "desktop = { theme = \"dark\" }") {
			t.Fatalf("config:\n%s", cfg)
		}
	})
}

// #2127 (cnlove7777777-art): a routing group Dash could run at High or Max
// showed in the Codex app at High with no effort control. Its entry lists
// high and max, but the app offers a model only the efforts in [desktop]
// enabled-reasoning-efforts, whose default leaves max out, and magpie
// added max only when Codex's config.toml was on one of magpie's models:
// the group got its levels later (8c5190a20, a catalog change) while the
// config was on Codex's own model, the app's last pick, so the catalog was
// rewritten with high and max and the app still offered high alone.
func TestCodexDesktopEffortsFollowTheCatalog(t *testing.T) {
	home, read := codexHome(t, "", "")
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "zai":{"models":{"glm-5.3":{"id":"glm-5.3","reasoning_options":[{"type":"effort","values":["low","high","max"]}]},
	                   "glm-4.5-air":{"id":"glm-4.5-air"}}},
	  "deepseek":{"models":{"deepseek-v4":{"id":"deepseek-v4","reasoning_options":[{"type":"effort","values":["high","max"]}]}}}}`), 0o644)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := provider.Save(provider.Provider{ID: "volc", Name: "Volc", Key: "k", Chat: "http://127.0.0.1:1/api/v3",
		Models: []string{"glm-4.5-air"}}); err != nil {
		t.Fatal(err)
	}
	cx := codex(home)
	if err := cx.Fields[0].Set("volc/glm-4.5-air"); err != nil {
		t.Fatal(err)
	}
	if cfg := read(); strings.Contains(cfg, `"max"`) {
		t.Fatalf("max enabled with no model taking it:\n%s", cfg)
	}
	// the app's own pick, written to config.toml as Codex writes it
	path := filepath.Join(home, ".codex", "config.toml")
	if err := edit.SetTOMLTop(path, edit.KV{Path: "model", Value: "gpt-5.5"}); err != nil {
		t.Fatal(err)
	}
	// the group comes to take high and max: its members are added
	if err := provider.Save(provider.Provider{ID: "volc", Name: "Volc", Key: "k", Chat: "http://127.0.0.1:1/api/v3",
		Models: []string{"glm-4.5-air", "glm-5.3", "deepseek-v4"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "normal", Name: "普通性能", Members: []string{"volc/glm-5.3", "volc/deepseek-v4"}}); err != nil {
		t.Fatal(err)
	}
	if e := catalog.Efforts(magpieModels("codex"), "group/normal"); strings.Join(e, ",") != "high,max" {
		t.Fatalf("group levels %v", e)
	}
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	cfg := read()
	if !strings.Contains(cfg, "[desktop]\nenabled-reasoning-efforts = [\"low\", \"medium\", \"high\", \"xhigh\", \"ultra\", \"persistent\", \"max\"]\n") {
		t.Fatalf("the app isn't offered max for the group:\n%s", cfg)
	}
	if !strings.Contains(cfg, `model = "gpt-5.5"`) {
		t.Fatalf("the app's pick changed:\n%s", cfg)
	}
}
