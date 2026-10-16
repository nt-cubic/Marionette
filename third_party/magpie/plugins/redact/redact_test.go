package redact

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

func TestMain(m *testing.M) { testenv.Main(m) }

func manifest(t *testing.T) pluginrt.Manifest {
	t.Helper()
	for _, m := range pluginrt.Registered() {
		if m.ID == "redact" {
			return m
		}
	}
	t.Fatal("redact not registered")
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

// On, the gateway's masker is served; off, none is; on again, it is back.
func TestOnServesOffDoesNot(t *testing.T) {
	sandbox(t)
	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	served := func(want bool) {
		t.Helper()
		if _, ok := gwmw.Served(); ok != want {
			t.Fatalf("masker served: %v, want %v (%+v)", ok, want, rt.Statuses())
		}
	}
	served(false)
	rt.Reconcile(map[string]bool{"redact": true}, nil)
	served(true)
	rt.Reconcile(map[string]bool{"redact": false}, nil)
	served(false)
	rt.Reconcile(map[string]bool{"redact": true}, nil)
	served(true)
}

// In use while the settings ask for anything to be masked: secrets,
// personal data or words of the user's own; settings that can't be read
// keep it on.
func TestInUse(t *testing.T) {
	sandbox(t)
	m := manifest(t)
	if n, err := m.InUse(); err != nil || n != 0 {
		t.Fatalf("no settings: %d %v", n, err)
	}
	for _, st := range []settings.Settings{{Redact: true}, {RedactPersonal: true}, {RedactWords: []string{"Project X"}}} {
		if err := settings.Save(st); err != nil {
			t.Fatal(err)
		}
		if n, err := m.InUse(); err != nil || n != 1 {
			t.Fatalf("%+v: %d %v", st, n, err)
		}
	}
	if err := settings.Save(settings.Settings{}); err != nil {
		t.Fatal(err)
	}
	if n, err := m.InUse(); err != nil || n != 0 {
		t.Fatalf("nothing asked for: %d %v", n, err)
	}
	if err := os.WriteFile(settings.Path(), []byte("{not json"), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, bak := range []string{".bak", ".lastgood"} {
		os.Remove(settings.Path() + bak)
	}
	if _, err := m.InUse(); err == nil {
		t.Fatal("unreadable settings counted as known")
	}
}
