package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// A model no catalog prices is at the price its plugin lists it at, and
// one the plugin says its plan serves free costs $0, not no price at all:
// Devin moved onto its plugin lists SWE-2 free (0.1.16, Devin's cost tier
// "Free") and, from 0.1.18, each paid model at Devin's own price, and
// calls on Devin's own models showed no cost on Usage (太陽と向日葵 on
// Discord, 2026-10-15). A model the plugin gives no price and doesn't say
// is free stays unpriced, and one a catalog prices keeps that price.
func TestPluginFreeModelCostsNothing(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "")
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	// models.dev as it has them (2026-10-15): Anthropic's Opus 5.5, and
	// Cognition's SWE models nowhere
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "anthropic":{"id":"anthropic","models":{"claude-opus-5-5":{"id":"claude-opus-5-5","cost":{"input":4,"output":20,"cache_read":0.2,"cache_write":5}}}}}`), 0o644)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	spec := "@magpie-community/opencode-devin-auth"
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		os.MkdirAll(settings.Dir(), 0o700)
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": spec}}})
	write("plugin-auth.json", map[string]map[string]any{"devin": {"type": "api", "key": "devin-key-1234"}})
	write("migrations.json", map[string]any{"devin": map[string]any{"state": "plugin", "package": spec}})
	// the ids the plugin lists (packages/devin listed(): a family's uid,
	// free when every variant Devin gives is in the Free tier)
	plugin.UseCached([]plugin.Provider{{ID: "devin", Spec: spec, Name: "Devin",
		Methods:  []plugin.Method{{Type: "api", Label: "API key"}},
		Accounts: []plugin.Account{{Key: "devin", Type: "api"}},
		Models: listedModels(t, `[
			{"id": "swe-2", "name": "SWE-2", "free": true, "cost": {"input": 0, "output": 0, "cache": {"read": 0, "write": 0}}},
			{"id": "swe-1.7-lightning", "name": "SWE-1.7 Lightning", "cost": {"input": 2.5, "output": 12.5, "cache": {"read": 1, "write": 0}}},
			{"id": "swe-1-6", "name": "SWE-1.6", "cost": {"input": 0, "output": 0, "cache": {"read": 0, "write": 0}}},
			{"id": "claude-opus-5-5", "name": "Claude Opus 5.5", "cost": {"input": 5, "output": 25, "cache": {"read": 0.5, "write": 0}}}]`),
	}})
	t.Cleanup(func() { plugin.UseCached(nil) })
	if !Moved("devin") {
		t.Fatal("devin not moved")
	}

	if pr, ok := EffectivePrice("devin", "swe-2"); !ok || pr.Input != 0 || pr.Output != 0 || pr.CacheRead != 0 || pr.CacheWrite != 0 {
		t.Errorf("swe-2: %+v, %v; want $0, priced", pr, ok)
	}
	// Devin's own price, as the plugin lists it
	if pr, ok := EffectivePrice("devin", "swe-1.7-lightning"); !ok || pr.Input != 2.5 || pr.Output != 12.5 || pr.CacheRead != 1 || pr.CacheWrite != 0 {
		t.Errorf("swe-1.7-lightning: %+v, %v; want Devin's $2.5/$12.5, cache read $1", pr, ok)
	}
	if pr, ok := EffectivePrice("devin", "swe-1-6"); ok {
		t.Errorf("swe-1-6: %+v, priced; the plugin gives it no price and doesn't say it is free", pr)
	}
	// a catalog's price is kept over the plugin's
	if pr, ok := EffectivePrice("devin", "claude-opus-5-5"); !ok || pr.Input != 4 || pr.Output != 20 {
		t.Errorf("claude-opus-5-5: %+v, %v; want Anthropic's $4/$20", pr, ok)
	}
	// another provider's model of the same id isn't the plugin's
	if pr, ok := EffectivePrice("anthropic", "swe-2"); ok {
		t.Errorf("anthropic/swe-2: %+v, priced", pr)
	}
}

// listedModels is a plugin's list as the host gives it (plugin.Model's JSON).
func listedModels(t *testing.T, s string) []plugin.Model {
	t.Helper()
	var ms []plugin.Model
	if err := json.Unmarshal([]byte(s), &ms); err != nil {
		t.Fatal(err)
	}
	return ms
}
