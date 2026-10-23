package gui

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

func TestPluginSubscriptionCarriesSingleIdentity(t *testing.T) {
	cfg := pluginsSandbox(t)
	if err := os.MkdirAll(cfg, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(cfg, "plugins.json"), []byte(`{"plugins":[{"spec":"public-fixture-plugin"}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	plugin.UseCached([]plugin.Provider{{ID: "public-fixture", Spec: "public-fixture-plugin", Name: "Public fixture", Single: true, Methods: []plugin.Method{{Type: "api", Keyless: true}}}})
	t.Cleanup(func() { plugin.UseCached(nil) })
	subs := pluginSubs()
	if len(subs) != 1 || !subs[0].Single || len(subs[0].Methods) != 1 || !subs[0].Methods[0].Keyless {
		t.Fatalf("subscription lost metadata: %+v", subs)
	}
}
