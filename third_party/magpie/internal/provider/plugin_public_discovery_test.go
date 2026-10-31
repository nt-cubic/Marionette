package provider

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// lileo0668 on Discord: with Settings → Privacy's subscription accounts
// off (#1535), OpenCode Zen Free's keyless sign-in went through, the
// Plugins page said it was signed in, and the providers listed it nowhere
// ("signed in, but magpie can't list it"). A plugin whose every way in is
// keyless holds no account of the user's: it is listed and served with
// them off. Any other plugin's key is refused then, as its browser
// sign-in is, rather than saved and listed nowhere.
func TestPublicPluginListedWithAccountsOff(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "") // nothing here runs a plugin
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		os.MkdirAll(settings.Dir(), 0o700)
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	zen, acme := "@magpie-community/opencode-zen-free-auth", "@example/opencode-acme-auth"
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": zen}, {"spec": acme}}})
	write("plugin-auth.json", map[string]map[string]any{
		"opencode-zen-free": {"type": "api", "key": "public"},
		"acme":              {"type": "api", "key": "sk-acme-1234"},
	})
	models := []plugin.Model{{ID: "big-pickle", Name: "Big Pickle", Free: true}}
	plugin.UseCached([]plugin.Provider{
		{ID: "opencode-zen-free", Spec: zen, Name: "OpenCode Zen Free", Single: true,
			Methods:  []plugin.Method{{Type: "api", Label: "Zen 免费访问", Keyless: true}},
			Accounts: []plugin.Account{{Key: "opencode-zen-free", Type: "api"}},
			Models:   models},
		// keyless one way, a browser the other: an account of the user's
		{ID: "acme", Spec: acme, Name: "Acme",
			Methods:  []plugin.Method{{Type: "api", Label: "API key", Keyless: true}, {Type: "oauth", Label: "Browser"}},
			Accounts: []plugin.Account{{Key: "acme", Type: "api"}},
			Models:   models},
	})
	t.Cleanup(func() { plugin.UseCached(nil) })

	listed := func() map[string]Provider {
		out := map[string]Provider{}
		for _, p := range All() {
			out[p.ID] = p
		}
		return out
	}
	// on, both are listed, as ever
	if got := listed(); !got["opencode-zen-free"].IsPlugin() || !got["acme"].IsPlugin() {
		t.Fatalf("with accounts on: %v", got)
	}

	before, _ := os.ReadFile(loginsPath())
	if err := settings.Save(settings.Settings{NoAccountDiscovery: true}); err != nil {
		t.Fatal(err)
	}
	got := listed()
	zp, ok := got["opencode-zen-free"]
	if !ok || !zp.IsPlugin() {
		t.Fatalf("Zen Free not listed with accounts off: %v", got)
	}
	if _, ok := got["acme"]; ok {
		t.Errorf("Acme, an account of the user's, listed with accounts off")
	}
	if zp.Name != "OpenCode Zen Free" || !zp.Account.Lists("big-pickle") {
		t.Errorf("Zen Free as listed: %q, lists big-pickle %v", zp.Name, zp.Account.Lists("big-pickle"))
	}
	if p, err := Find("opencode-zen-free"); err != nil || !p.IsPlugin() {
		t.Errorf("Find(opencode-zen-free) = %+v, %v", p, err)
	}
	req, _ := http.NewRequest("POST", "http://127.0.0.1/v1/chat/completions", nil)
	if err := zp.Sign(context.Background(), req, Chat, nil); err != nil {
		t.Errorf("Zen Free not served with accounts off: %v", err)
	}

	// a key to Acme is refused before anything is saved; Zen Free's goes
	// on to the plugin (no Bun here, so it fails there, not on the gate)
	if _, err := PluginAPIKey(context.Background(), "acme", 0, nil, "sk-acme-5678"); !errors.Is(err, errAccountDiscoveryOff) {
		t.Errorf("Acme's key with accounts off: %v, want %v", err, errAccountDiscoveryOff)
	}
	if _, err := PluginAPIKey(context.Background(), "opencode-zen-free", 0, nil, ""); errors.Is(err, errAccountDiscoveryOff) {
		t.Errorf("Zen Free's keyless sign-in refused with accounts off")
	}
	if after, _ := os.ReadFile(loginsPath()); string(after) != string(before) {
		t.Errorf("logins.json written with accounts off:\n%s\nwas\n%s", after, before)
	}
}
