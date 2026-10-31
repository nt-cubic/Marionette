package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// ccPlugin is the Command Code plan as its plugin serves it: its
// requests go to plugin://, which names no vendor (treeman on Discord).
var ccPlugin = Provider{ID: CommandCodePlanID, Chat: pluginBase + CommandCodePlanID + "/v1", Responses: pluginBase + CommandCodePlanID + "/v1"}

// Only the vendors whose own pricing bills DeepSeek's models by the hour
// are: DeepSeek's API, Command Code's DeepSeek V4 models (not its flat
// deepseek-v4-flash-fast) and OpenCode Go's two. OpenCode Zen, a relay, a
// lookalike host and every other model keep one price.
func TestPeakOf(t *testing.T) {
	preset := func(id string) Provider {
		p, err := FromPreset(id)
		if err != nil {
			t.Fatal(err)
		}
		return p
	}
	for _, c := range []struct {
		name  string
		p     Provider
		model string
		want  catalog.Peak
	}{
		{"deepseek", preset("deepseek"), "deepseek-flash", catalog.PeakDeepSeek},
		{"deepseek pro", preset("deepseek"), "deepseek-v4-pro", catalog.PeakDeepSeek},
		{"commandcode", preset("commandcode"), "deepseek/deepseek-v4.1-flash", catalog.PeakWeekdays},
		{"commandcode pro", preset("commandcode"), "deepseek/deepseek-v4-pro", catalog.PeakWeekdays},
		{"commandcode flash-fast", preset("commandcode"), "deepseek/deepseek-v4-flash-fast", ""},
		{"commandcode other", preset("commandcode"), "moonshotai/kimi-k3", ""},
		{"commandcode plan", Provider{ID: CommandCodePlanID}, "deepseek/deepseek-v4.1-flash", catalog.PeakWeekdays},
		{"commandcode plan's plugin", ccPlugin, "deepseek/deepseek-v4.1-flash", catalog.PeakWeekdays},
		{"commandcode plan's plugin flash-fast", ccPlugin, "deepseek/deepseek-v4-flash-fast", ""},
		{"another plugin", Provider{ID: "zen-free", Chat: pluginBase + "zen-free/v1"}, "deepseek-v4.1-flash", ""},
		{"opencode go", preset("opencode-go"), "deepseek-v4.1-flash", catalog.PeakWeekdays},
		{"opencode go other", preset("opencode-go"), "kimi-k3", ""},
		{"opencode zen", preset("opencode-zen"), "deepseek-v4.1-flash", ""},
		{"relay", Provider{ID: "relay", Chat: "https://relay.example/v1"}, "deepseek-v4.1-flash", ""},
		{"relay named deepseek", Provider{ID: "deepseek", Preset: "deepseek", Chat: "https://relay.example/v1"}, "deepseek-flash", ""},
		{"lookalike host", Provider{ID: "x", Chat: "https://api.deepseek.com.evil.example/v1"}, "deepseek-flash", ""},
		{"removed deepseek", Provider{ID: "deepseek"}, "deepseek-flash", catalog.PeakDeepSeek},
		{"openai", preset("openai"), "gpt-6-astra", ""},
	} {
		if got := c.p.PeakOf(c.model); got != c.want {
			t.Errorf("%s %s: %q, want %q", c.name, c.model, got, c.want)
		}
	}
}

// useDeepSeekCatalog puts a catalog in the cache that lists DeepSeek's
// deepseek-flash at its off-peak price, as models.dev's api.json does
// (its peak multiplier left out), OpenCode Zen's deepseek-v4.1-flash at
// its flat price and an OpenAI model: no network.
func useDeepSeekCatalog(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	model := func(id string, in, out float64) map[string]any {
		return map[string]any{"id": id, "cost": map[string]any{"input": in, "output": out}}
	}
	b, _ := json.Marshal(map[string]any{
		"deepseek": map[string]any{"id": "deepseek", "models": map[string]any{
			"deepseek-flash": model("deepseek-flash", 0.15, 0.6), "deepseek-v4.1-flash": model("deepseek-v4.1-flash", 0.15, 0.6)}},
		"opencode": map[string]any{"id": "opencode", "models": map[string]any{
			"deepseek-v4.1-flash": model("deepseek-v4.1-flash", 0.3, 1.2)}},
		"openai": map[string]any{"id": "openai", "models": map[string]any{
			"gpt-6-astra": model("gpt-6-astra", 2, 8)}},
	})
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), b, 0o600); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
}

// A DeepSeek model's list price doubles at DeepSeek's peak hours, at the
// provider's price rate still, and so does a price the user set for it:
// the vendor doubles whatever its off-peak price is. OpenCode Zen's flat
// price and another vendor's model hold at every hour, set or not.
func TestPriceOfPeak(t *testing.T) {
	useDeepSeekCatalog(t)
	peak := time.Date(2026, 10, 29, 2, 0, 0, 0, time.UTC) // a Thursday
	off := time.Date(2026, 10, 29, 11, 0, 0, 0, time.UTC)
	preset := func(id string) Provider {
		p, err := FromPreset(id)
		if err != nil {
			t.Fatal(err)
		}
		return p
	}
	at := func(s settings.Settings, p Provider, model string, tm time.Time) float64 {
		t.Helper()
		pr, ok := priceOf(s, p, true, model, time.Time{})
		if !ok {
			t.Fatalf("%s %s: unpriced", p.ID, model)
		}
		return pr.In(tm).Input
	}
	none := settings.Settings{}
	ds := preset("deepseek")
	if a, b := at(none, ds, "deepseek-flash", peak), at(none, ds, "deepseek-flash", off); a != 0.3 || b != 0.15 {
		t.Errorf("deepseek-flash: %v at peak, %v off-peak; want 0.3, 0.15", a, b)
	}
	half := ds
	half.PriceRate = 0.5
	if a := at(none, half, "deepseek-flash", peak); a != 0.15 {
		t.Errorf("deepseek-flash at a 0.5 rate: %v at peak, want 0.15", a)
	}
	cc := preset("commandcode") // no catalog of its own: DeepSeek's price
	if a, b := at(none, cc, "deepseek/deepseek-v4.1-flash", peak), at(none, cc, "deepseek/deepseek-v4.1-flash", off); a != 0.3 || b != 0.15 {
		t.Errorf("commandcode deepseek-v4.1-flash: %v at peak, %v off-peak", a, b)
	}
	zen := preset("opencode-zen")
	if a, b := at(none, zen, "deepseek-v4.1-flash", peak), at(none, zen, "deepseek-v4.1-flash", off); a != 0.3 || b != 0.3 {
		t.Errorf("opencode zen deepseek-v4.1-flash: %v at peak, %v off-peak; want 0.3 both", a, b)
	}
	oa := preset("openai")
	if a, b := at(none, oa, "gpt-6-astra", peak), at(none, oa, "gpt-6-astra", off); a != 2 || b != 2 {
		t.Errorf("gpt-6-astra: %v at peak, %v off-peak; want 2 both", a, b)
	}
	// the user's own price for the model, or for every model of DeepSeek
	in, out, cache := 0.2, 0.8, 0.02
	for _, key := range []string{"deepseek/deepseek-flash", "deepseek/*", AnyPriceKey("deepseek-flash")} {
		s := settings.Settings{ModelPrices: map[string]settings.ModelPrice{key: {Input: &in, Output: &out, CacheRead: &cache, CacheWrite: &cache}}}
		if a, b := at(s, ds, "deepseek-flash", peak), at(s, ds, "deepseek-flash", off); a != 0.4 || b != 0.2 {
			t.Errorf("set at %s: %v at peak, %v off-peak; want 0.4, 0.2", key, a, b)
		}
	}
	for _, c := range []struct {
		p     Provider
		model string
	}{{zen, "deepseek-v4.1-flash"}, {oa, "gpt-6-astra"}} {
		for _, key := range []string{c.p.ID + "/" + c.model, AnyPriceKey(c.model)} {
			s := settings.Settings{ModelPrices: map[string]settings.ModelPrice{key: {Input: &in, Output: &out, CacheRead: &cache, CacheWrite: &cache}}}
			if a, b := at(s, c.p, c.model, peak), at(s, c.p, c.model, off); a != 0.2 || b != 0.2 {
				t.Errorf("%s set at %s: %v at peak, %v off-peak; want 0.2 both", c.p.ID, key, a, b)
			}
		}
	}
	// a provider gone since, priced at its maker's
	if pr, ok := priceOf(none, Provider{ID: "deepseek"}, false, "deepseek-flash", time.Time{}); !ok || pr.In(peak).Input != 0.3 {
		t.Errorf("removed deepseek: %+v %v", pr, ok)
	}
}

// commandCodeCatalog is models.dev's api.json as magpie had it on
// 2026-10-30, these entries byte for byte (descriptions and the like left
// out). DeepSeek lists V4.1 Flash only as deepseek-flash, by a dated
// canonical id, so no maker price is found for Command Code's undated
// deepseek/deepseek-v4.1-flash; Command Code's own catalog lists it.
const commandCodeCatalog = `{
"deepseek":{"id":"deepseek","api":"https://api.deepseek.com","name":"DeepSeek","models":{
"deepseek-flash":{"id":"deepseek-flash","name":"DeepSeek Flash","cost":{"input":0.15,"output":0.6,"cache_read":0.003},"canonical_model_id":"deepseek/deepseek-v4.1-flash-1017"},
"deepseek-v4-pro":{"id":"deepseek-v4-pro","name":"DeepSeek V4 Pro","cost":{"input":0.66,"output":1.98,"cache_read":0.013},"canonical_model_id":"deepseek/deepseek-v4-pro"}}},
"commandcode":{"id":"commandcode","env":["CMD_API_KEY"],"npm":"@ai-sdk/openai-compatible","api":"https://api.commandcode.ai/provider/v1","name":"CommandCode","doc":"https://commandcode.ai/docs/provider","models":{
"deepseek/deepseek-v4.1-flash":{"id":"deepseek/deepseek-v4.1-flash","name":"DeepSeek V4.1 Flash","family":"deepseek-flash","cost":{"input":0.15,"output":0.6,"cache_read":0.003},"canonical_model_id":"deepseek/deepseek-v4.1-flash"},
"deepseek/deepseek-v4-pro":{"id":"deepseek/deepseek-v4-pro","name":"DeepSeek V4 Pro","cost":{"input":0.9,"output":2.7,"cache_read":0.02},"canonical_model_id":"deepseek/deepseek-v4-pro"}}}
}`

// The Command Code plan, served by its plugin, prices DeepSeek V4.1 Flash
// at Command Code's list price, doubled in its peak hours, where it showed
// no price at all and its calls were counted at nothing (treeman on
// Discord). A model DeepSeek prices keeps DeepSeek's price, and a price
// the user set is doubled at peak too.
func TestCommandCodePlanPrice(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), []byte(commandCodeCatalog), 0o600); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	peak := time.Date(2026, 10, 29, 2, 0, 0, 0, time.UTC) // a Thursday
	off := time.Date(2026, 10, 29, 11, 0, 0, 0, time.UTC)
	price := func(s settings.Settings, model string) catalog.Price {
		t.Helper()
		pr, ok := priceOf(s, ccPlugin, true, model, time.Time{})
		if !ok {
			t.Fatalf("%s: unpriced", model)
		}
		return pr
	}
	none := settings.Settings{}
	if pr, ok := ccPlugin.ListPrice("deepseek/deepseek-v4.1-flash"); !ok || pr.Input != 0.15 || pr.Output != 0.6 || pr.CacheRead != 0.003 {
		t.Errorf("list price: %+v %v; want 0.15/0.6/0.003", pr, ok)
	}
	pr := price(none, "deepseek/deepseek-v4.1-flash")
	if a, b := pr.In(peak), pr.In(off); a.Input != 0.3 || a.Output != 1.2 || b.Input != 0.15 || b.Output != 0.6 {
		t.Errorf("deepseek-v4.1-flash: %+v at peak, %+v off-peak", a, b)
	}
	// DeepSeek's own price comes first, as before
	if pr := price(none, "deepseek/deepseek-v4-pro"); pr.In(off).Input != 0.66 || pr.In(peak).Input != 1.32 {
		t.Errorf("deepseek-v4-pro: %+v", pr)
	}
	in, out, cache := 0.1, 0.4, 0.0
	for _, key := range []string{CommandCodePlanID + "/deepseek/deepseek-v4.1-flash", AnyPriceKey("deepseek/deepseek-v4.1-flash")} {
		s := settings.Settings{ModelPrices: map[string]settings.ModelPrice{key: {Input: &in, Output: &out, CacheRead: &cache, CacheWrite: &cache}}}
		pr := price(s, "deepseek/deepseek-v4.1-flash")
		if a, b := pr.In(peak), pr.In(off); a.Input != 0.2 || a.Output != 0.8 || b.Input != 0.1 || b.Output != 0.4 {
			t.Errorf("set at %s: %+v at peak, %+v off-peak", key, a, b)
		}
	}
	// another provider's id says nothing of Command Code's catalog
	if _, ok := (Provider{ID: "relay", Chat: "https://relay.example/v1"}).ListPrice("deepseek/deepseek-v4.1-flash"); ok {
		t.Error("a relay took Command Code's list price")
	}
}
