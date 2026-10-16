package groups

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

func TestMain(m *testing.M) { testenv.Main(m) }

func manifest(t *testing.T) pluginrt.Manifest {
	t.Helper()
	for _, m := range pluginrt.Registered() {
		if m.ID == "groups" {
			return m
		}
	}
	t.Fatal("groups not registered")
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

// Where the gateway is served, the user's own groups are served while the
// plugin is on, and not while it is off.
func TestOnServesOffDoesNot(t *testing.T) {
	sandbox(t)
	defer provider.GateGroups(nil)()
	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	served := func(want bool) {
		t.Helper()
		if provider.GroupsServed() != want {
			t.Fatalf("served: %v, want %v (%+v)", !want, want, rt.Statuses())
		}
	}
	served(false)
	rt.Reconcile(map[string]bool{"groups": true}, nil)
	served(true)
	rt.Reconcile(map[string]bool{"groups": false}, nil)
	served(false)
	rt.Reconcile(map[string]bool{"groups": true}, nil)
	served(true)
}

// It counts the user's own groups; a providers.json that can't be read
// keeps it on.
func TestInUse(t *testing.T) {
	sandbox(t)
	inUse := manifest(t).InUse
	if n, err := inUse(); n != 0 || err != nil {
		t.Fatalf("no file: %d, %v", n, err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Mine", Members: []string{"a/x", "b/y"}}); err != nil {
		t.Fatal(err)
	}
	if n, err := inUse(); n != 1 || err != nil {
		t.Fatalf("one group: %d, %v", n, err)
	}
	if err := os.WriteFile(provider.Path(), []byte("{not json"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := inUse(); err == nil {
		t.Fatal("no error for a providers.json that can't be read")
	}
}
