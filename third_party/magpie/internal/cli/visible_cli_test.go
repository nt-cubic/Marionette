package cli

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// The visibility is kept under the agent's id lowered, because VisibleTo
// looks it up lowered: a WSL id's case must not split the key the write
// makes from the key the read asks for, and the one all takes back out is
// the same key again.
func TestVisibleKeyLowered(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { provider.Delete("relay") })
	if err := visibleCmd([]string{"CODEX", "relay"}); err != nil {
		t.Fatal(err)
	}
	if _, ok := settings.Load().Visible["codex"]; !ok {
		t.Fatalf("kept under a key that is not the id lowered: %v", settings.Load().Visible)
	}
	if names, ok := provider.VisibleTo("CODEX"); !ok || len(names) != 1 || names[0] != "relay" {
		t.Fatalf("VisibleTo: %v, %v", names, ok)
	}
	if err := visibleCmd([]string{"CODEX", "all"}); err != nil {
		t.Fatal(err)
	}
	if _, ok := settings.Load().Visible["codex"]; ok {
		t.Fatal("all took the visibility out under another key than the id lowered")
	}
}

// magpie models and magpie visible take a WSL agent by its id, in the
// distro's own case or any other: before, both lowered what was typed and
// then matched it exactly against ids that keep the distro's case (#271).
func TestWSLAgentByIDInModelsAndVisible(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(t.TempDir(), "config"))
	root := t.TempDir()
	h := filepath.Join(root, "home", "me", ".claude")
	if err := os.MkdirAll(h, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(h, "settings.json"), []byte("{}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(agent.FakeWSL(map[string]string{"Ubuntu": "home:/home/me\ndir:.claude\n"}, map[string]string{"Ubuntu": root}))
	for _, q := range []string{"claude@wsl:Ubuntu", "claude@wsl:ubuntu", "CLAUDE@WSL:UBUNTU"} {
		if err := models([]string{q}); err != nil {
			t.Errorf("models %q: %v", q, err)
		}
		if err := visibleCmd([]string{q}); err != nil {
			t.Errorf("visible %q: %v", q, err)
		}
	}
}
