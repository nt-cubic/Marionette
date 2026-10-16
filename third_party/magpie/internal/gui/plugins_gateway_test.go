package gui

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

func TestGatewayPluginOptionsAndKind(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	t.Cleanup(plugin.Settle)
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"name":"gateway-fixture","magpie":{"hooks":"./hooks.js","options":{"serverUrl":"http://127.0.0.1:8000"}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "hooks.js"), []byte(`export function onRequest() {}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Add(t.Context(), dir); err != nil {
		t.Fatal(err)
	}
	state := pluginsState(t.Context(), nil)
	if len(state.Plugins) != 1 {
		t.Fatalf("plugins: %+v", state)
	}
	p := state.Plugins[0]
	if !p.IsGateway || !p.GatewayOnly || p.OptionsExample["serverUrl"] != "http://127.0.0.1:8000" || p.Gateway == nil || p.Gateway.LastOutcome != "idle" {
		t.Fatalf("gateway missing from Settings: %+v", p)
	}
	if err := plugin.SetOff(dir, true); err != nil {
		t.Fatal(err)
	}
	p = pluginsState(t.Context(), nil).Plugins[0]
	if !p.IsGateway || p.Gateway != nil {
		t.Fatalf("disabled gateway: %+v", p)
	}
}
