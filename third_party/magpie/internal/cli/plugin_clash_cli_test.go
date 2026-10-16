package cli

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// magpie plugin lists both plugins that sign in to one provider (neiko on
// Discord: a third-party plugin and their own), says which one serves it
// and how to have the other one serve it; `magpie plugin use` does that.
func TestPluginListSaysClash(t *testing.T) {
	bun := testenv.Bun(t)
	groupsHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	t.Setenv("NO_COLOR", "1")
	t.Cleanup(plugin.Settle)
	src, err := os.ReadFile("../plugin/testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	mine, theirs := filepath.Join(dir, "mine", "index.js"), filepath.Join(dir, "theirs", "index.js")
	for _, f := range []string{mine, theirs} {
		os.MkdirAll(filepath.Dir(f), 0o755)
		if err := os.WriteFile(f, src, 0o644); err != nil {
			t.Fatal(err)
		}
		if _, err := plugin.Add(context.Background(), f); err != nil {
			t.Fatal(err)
		}
	}
	out, err := stdoutOf(t, func() error { return listPlugins(context.Background(), false) })
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "served by "+theirs+", which signs in to it too · magpie plugin use "+mine+" fakeco") {
		t.Errorf("mine's row doesn't say theirs serves fakeco:\n%s", out)
	}
	if !strings.Contains(out, "also signed in to by "+mine+"; this one serves it") {
		t.Errorf("theirs' row doesn't say it serves fakeco:\n%s", out)
	}
	out, err = stdoutOf(t, func() error { return pluginCmd([]string{"plugin", "use", mine, "fakeco"}) })
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "served by "+mine+", which signs in to it too · magpie plugin use "+theirs+" fakeco") {
		t.Errorf("after use, theirs' row doesn't say mine serves fakeco:\n%s", out)
	}
	if _, err := stdoutOf(t, func() error { return pluginCmd([]string{"plugin", "use", "nosuch", "fakeco"}) }); err == nil {
		t.Error("use took a plugin that isn't installed")
	}
}
