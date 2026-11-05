package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// laomao04_65777 on Discord: in Codex, SenseTime Raccoon's (商汤小浣熊's)
// models had no reasoning effort to pick. Its plugin lists sn-kimi-k3,
// sn-glm-5-3 and sn-deepseek-v4-1-flash as thinking with no variants;
// models.dev knows each under its maker's spelling, so the entry counted
// as known (levelsUnknown) and was offered none, and nothing borrowed the
// maker's levels as a custom provider's same id does. A plugin's thinking
// model with no variants borrows them now; a model the plugin gives
// variants keeps them, and one it doesn't say thinks gets none borrowed.
func TestPluginThinkingModelBorrowsEfforts(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "")
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "moonshotai":{"id":"moonshotai","models":{"kimi-k3":{"id":"kimi-k3","reasoning":true,
	    "reasoning_options":[{"type":"effort","values":["low","medium","high","max"]}],"limit":{"context":1000000}}}},
	  "zai":{"id":"zai","models":{"glm-5.3":{"id":"glm-5.3","reasoning":true,
	    "reasoning_options":[{"type":"effort","values":["low","high","max"]}],"limit":{"context":1000000}}}}}`), 0o644)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	spec := "@magpie-community/opencode-raccoon-auth"
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		os.MkdirAll(settings.Dir(), 0o700)
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": spec}}})
	write("plugin-auth.json", map[string]map[string]any{"raccoon": {"type": "oauth", "access": "a", "refresh": "r", "expires": 9999999999999}})
	plugin.UseCached([]plugin.Provider{{ID: "raccoon", Spec: spec, Name: "商汤小浣熊",
		Methods:  []plugin.Method{{Type: "oauth", Label: "Sign in"}},
		Accounts: []plugin.Account{{Key: "raccoon", Type: "oauth"}},
		Models: []plugin.Model{
			{ID: "sn-kimi-k3", Name: "Kimi-K3", Reasoning: true},
			{ID: "sn-glm-5-3", Name: "GLM-5-3", Reasoning: true, Variants: []string{"high"}},
			{ID: "kimi-k3", Name: "Kimi K3 (no word on thinking)"},
		}}})
	t.Cleanup(func() { plugin.UseCached(nil) })

	shown, _ := CatalogFor("codex")
	got := map[string][]string{}
	for _, m := range CodexCatalog(shown) {
		got[m.ID] = m.Efforts
	}
	want := map[string][]string{
		"raccoon/sn-kimi-k3": {"low", "medium", "high", "max"},
		"raccoon/sn-glm-5-3": {"high"},
		"raccoon/kimi-k3":    nil,
	}
	for id, w := range want {
		g, ok := got[id]
		if !ok {
			t.Fatalf("no %s in Codex's list: %v", id, got)
		}
		if !slices.Equal(g, w) {
			t.Errorf("%s offered %v in Codex, want %v", id, g, w)
		}
	}
	// the gateway fits an effort to the same levels
	p, err := Find("raccoon")
	if err != nil {
		t.Fatal(err)
	}
	if e := p.Efforts("sn-kimi-k3"); !slices.Equal(e, want["raccoon/sn-kimi-k3"]) {
		t.Errorf("provider's sn-kimi-k3 efforts %v", e)
	}
}
