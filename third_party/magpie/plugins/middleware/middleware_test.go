package middleware

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/pluginrt"
	"github.com/yetone/magpie/internal/routes"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/usage"
)

func TestMain(m *testing.M) { testenv.Main(m) }

func manifest(t *testing.T) pluginrt.Manifest {
	t.Helper()
	for _, m := range pluginrt.Registered() {
		if m.ID == "middleware" {
			return m
		}
	}
	t.Fatal("middleware not registered")
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

// On, the JS middleware chain is the gateway's agent middleware; off, the
// gateway has none; on again, it is back.
func TestOnServesOffDoesNot(t *testing.T) {
	sandbox(t)
	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	served := func(want bool) {
		t.Helper()
		if got := len(gwmw.Agents()) == 1 && gwmw.Agents()[0] == gwmw.Agent(middleware.Chain{}); got != want || !want && len(gwmw.Agents()) != 0 {
			t.Fatalf("chain served: %v, want %v (%+v)", gwmw.Agents(), want, rt.Statuses())
		}
	}
	served(false)
	rt.Reconcile(map[string]bool{"middleware": true}, nil)
	served(true)
	rt.Reconcile(map[string]bool{"middleware": false}, nil)
	served(false)
	rt.Reconcile(map[string]bool{"middleware": true}, nil)
	served(true)
}

// In use while plugins.json has a middleware on.
func TestInUse(t *testing.T) {
	sandbox(t)
	t.Cleanup(plugin.Settle)
	m := manifest(t)
	middleware.Reload()
	if n, err := m.InUse(); err != nil || n != 0 {
		t.Fatalf("no plugins: %d %v", n, err)
	}
	mw := filepath.Join(t.TempDir(), "a.middleware.js")
	os.WriteFile(mw, []byte(`export function onRequest(body) { return body; }`), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
	middleware.Reload()
	defer middleware.Reload()
	if n, err := m.InUse(); err != nil || n != 1 {
		t.Fatalf("one middleware: %d %v", n, err)
	}
}

// On, while the gateway serves, a JS gateway plugin's strategies are
// routings and its onUsage is an observer; off, neither (a pool routed by
// one goes in its listed order); on again, they are back.
func TestOnServesStrategies(t *testing.T) {
	sandbox(t)
	t.Cleanup(plugin.Settle)
	gw := filepath.Join(t.TempDir(), "a.gateway.js")
	os.WriteFile(gw, []byte(`export const strategies = { x: (p, s) => s.map((_, i) => i) }; export function onUsage(row) {}`), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: gw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
	middleware.Reload()
	defer middleware.Reload()
	stop := usage.ServeObservers()
	defer stop()
	rt := pluginrt.New([]pluginrt.Manifest{manifest(t)})
	defer rt.Stop()
	if routes.Served("a/x") {
		t.Fatal("served before the plugin is on")
	}
	rt.Reconcile(map[string]bool{"middleware": true}, nil)
	if !routes.Served("a/x") || len(usage.Observers()) != 1 {
		t.Fatalf("on: served %v, observers %d", routes.Served("a/x"), len(usage.Observers()))
	}
	rt.Reconcile(map[string]bool{"middleware": false}, nil)
	if routes.Served("a/x") || len(usage.Observers()) != 0 {
		t.Fatal("off: still served")
	}
	rt.Reconcile(map[string]bool{"middleware": true}, nil)
	if !routes.Served("a/x") {
		t.Fatal("on again: not served")
	}
}
