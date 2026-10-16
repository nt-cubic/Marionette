package tools

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/gwtool"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/testenv"
)

func TestMain(m *testing.M) { testenv.Main(m) }

func manifest(t *testing.T) pluginrt.Manifest {
	t.Helper()
	for _, m := range pluginrt.Registered() {
		if m.ID == "tools" {
			return m
		}
	}
	t.Fatal("tools not registered")
	return pluginrt.Manifest{}
}

func sandbox(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, "config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, "cache"))
}

// In a process serving the gateway, on serves the plugins' tools, off
// doesn't, on again does; a process not serving it has them either way.
func TestOnServesOffDoesNot(t *testing.T) {
	sandbox(t)
	if !gwtool.Served(gwtool.PluginTools) {
		t.Fatal("not served outside the gateway")
	}
	stop := gwtool.Gate()
	defer stop()
	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	served := func(want bool) {
		t.Helper()
		if ok := gwtool.Served(gwtool.PluginTools); ok != want {
			t.Fatalf("tools served: %v, want %v (%+v)", ok, want, rt.Statuses())
		}
	}
	served(false)
	rt.Reconcile(map[string]bool{"tools": true}, nil)
	served(true)
	rt.Reconcile(map[string]bool{"tools": false}, nil)
	served(false)
	rt.Reconcile(map[string]bool{"tools": true}, nil)
	served(true)
}

// In use for each plugin on in plugins.json that gave tools when the host
// last loaded it: one off, or one that gave none, isn't counted; no note
// yet is none; a note that can't be read is not known.
func TestInUse(t *testing.T) {
	sandbox(t)
	inUse := manifest(t).InUse
	count := func(want int) {
		t.Helper()
		if n, err := inUse(); err != nil || n != want {
			t.Fatalf("in use %d %v, want %d", n, err, want)
		}
	}
	count(0)
	write := func(p, s string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(s), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write(plugin.ListPath(), `{"plugins":[{"spec":"opencode-a"},{"spec":"opencode-b","off":true},{"spec":"opencode-c"},{"spec":"opencode-d"}]}`)
	count(0)
	write(plugin.ToolsPath(), `{"opencode-a":["x"],"opencode-b":["y"],"opencode-c":[]}`)
	count(1)
	write(plugin.ToolsPath(), `{"opencode-a":["x"],"opencode-b":["y"],"opencode-d":["z","w"]}`)
	count(2)
	write(plugin.ToolsPath(), `{"opencode-a":`)
	if _, err := inUse(); err == nil {
		t.Fatal("a half-written note counted as known")
	}
}
