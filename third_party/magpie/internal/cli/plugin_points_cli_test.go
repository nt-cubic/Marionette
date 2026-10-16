package cli

import (
	"context"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// magpie plugin says the tools a plugin gives agents, by their names on
// the plugin tools MCP server, and the page it adds to the window with
// what that page may read: the list had neither, so a plugin with only
// tools or a page had nothing under it.
func TestPluginListSaysToolsAndPage(t *testing.T) {
	bun := testenv.Bun(t)
	groupsHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	dir := t.TempDir()
	tools, page := filepath.Join(dir, "tools"), filepath.Join(dir, "page")
	if err := os.CopyFS(tools, os.DirFS("../plugin/testdata/tools")); err != nil {
		t.Fatal(err)
	}
	if err := os.CopyFS(page, os.DirFS("../gui/testdata/page-plugin")); err != nil {
		t.Fatal(err)
	}
	for _, spec := range []string{tools, page} {
		if _, err := plugin.Add(context.Background(), spec); err != nil {
			t.Fatal(err)
		}
	}
	out, err := stdoutOf(t, func() error { return listPlugins(context.Background(), false) })
	if err != nil {
		t.Fatal(err)
	}
	if !regexp.MustCompile(`(?m)^  tools  \S+_add, \S+_fail, \S+_shout, \S+_sleep$`).MatchString(out) {
		t.Errorf("the plugin's tools aren't listed by their MCP names:\n%s", out)
	}
	if !strings.Contains(out, "page  Usage glance (reads usage:summary)") {
		t.Errorf("the plugin's page isn't listed with what it reads:\n%s", out)
	}
	// switched off, the host doesn't load it: no tools are served or said
	if err := pluginCmd([]string{"plugin", "off", tools}); err != nil {
		t.Fatal(err)
	}
	out, _ = stdoutOf(t, func() error { return listPlugins(context.Background(), false) })
	if strings.Contains(out, "_shout") {
		t.Errorf("a plugin switched off still lists its tools:\n%s", out)
	}
}
