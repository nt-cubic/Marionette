package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// With CODEX_HOME set, a model picked for Codex goes to
// $CODEX_HOME/config.toml, the file Codex reads, and its catalog beside it;
// nothing is written under ~/.codex, and what magpie shows as applied is
// read back from there (Wakkana on Discord, Codex 0.162.1: the pick went to
// ~/.codex/config.toml, Codex never saw it, and magpie showed it applied).
func TestCodexHomeEnvTakesThePick(t *testing.T) {
	home, _ := codexHome(t, "", "")
	if err := os.RemoveAll(filepath.Join(home, ".codex")); err != nil {
		t.Fatal(err)
	}
	ch := filepath.Join(t.TempDir(), "codex-home")
	os.MkdirAll(ch, 0o755)
	os.WriteFile(filepath.Join(ch, "config.toml"), []byte("model = \"gpt-5.5\"\n"), 0o644)
	t.Setenv("CODEX_HOME", ch)

	cx := codex(home)
	if cx.Path != filepath.Join(ch, "config.toml") || cx.Dir != ch {
		t.Fatalf("Codex's files at %s (%s), want under %s", cx.Path, cx.Dir, ch)
	}
	if err := cx.Fields[0].Set("fake/m1"); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(filepath.Join(ch, "config.toml"))
	if cfg := string(b); !strings.Contains(cfg, `model = "fake/m1"`) || !strings.Contains(cfg, "magpie-models.json") {
		t.Fatalf("$CODEX_HOME/config.toml doesn't have the pick:\n%s", cfg)
	}
	if _, err := os.Stat(filepath.Join(ch, "magpie-models.json")); err != nil {
		t.Fatalf("the catalog isn't beside it: %v", err)
	}
	if got := codex(home).Fields[0].Get(); got != "fake/m1" {
		t.Fatalf("magpie reads the pick back as %q", got)
	}
	if m := StandIn("codex", "gpt-5"); m != "fake/m1" {
		t.Fatalf("the gateway's stand-in for Codex is %q, read from somewhere else", m)
	}
	if err := TuneApply(home, "codex", TuneCompact, "", "200000"); err != nil {
		t.Fatal(err)
	}
	b, _ = os.ReadFile(filepath.Join(ch, "config.toml"))
	if !strings.Contains(string(b), "model_auto_compact_token_limit = 200000") {
		t.Fatalf("tune didn't write $CODEX_HOME/config.toml:\n%s", b)
	}
	if _, err := os.Stat(filepath.Join(home, ".codex")); !os.IsNotExist(err) {
		var names []string
		filepath.WalkDir(filepath.Join(home, ".codex"), func(p string, _ os.DirEntry, _ error) error {
			names = append(names, p)
			return nil
		})
		t.Fatalf("written under ~/.codex with CODEX_HOME set: %v", names)
	}
}
