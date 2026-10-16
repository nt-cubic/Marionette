package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// useXiaomiCatalog puts testdata/xiaomi_catalog.json in the catalog cache:
// models.dev's xiaomi and its three Token Plan catalogs (2026-10-13; id,
// name, cost and canonical id of each model), the plans listing every model
// at $0; plus a relay's catalog that lists mimo-v2.6-flash at $0 of its own.
func useXiaomiCatalog(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	b, err := os.ReadFile(filepath.Join("testdata", "xiaomi_catalog.json"))
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	m["freebie"] = map[string]any{"id": "freebie", "models": map[string]any{
		"mimo-v2.6-flash": map[string]any{"id": "mimo-v2.6-flash", "cost": map[string]any{"input": 0, "output": 0}}}}
	b, _ = json.Marshal(m)
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), b, 0o600); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
}

// xiaomiAt is a Xiaomi MiMo provider at that region of the preset, with
// the catalog given, as it reads back once saved.
func xiaomiAt(t *testing.T, region, cat string) Provider {
	t.Helper()
	pr := Preset("xiaomi")
	for _, r := range pr.Regions {
		if r.ID == region {
			return normalize(Provider{ID: "xiaomi", Name: "Xiaomi MiMo", Preset: "xiaomi", Key: "k",
				Chat: r.Chat, Responses: r.Responses, Anthropic: r.Anthropic, Catalog: cat})
		}
	}
	t.Fatalf("xiaomi has no region %q", region)
	return Provider{}
}

var (
	mimoFlash = catalog.Price{Input: 0.14, Output: 0.28, CacheRead: 0.0028}
	mimoPro   = catalog.Price{Input: 0.435, Output: 0.87, CacheRead: 0.0036}
)

// #1873: a Xiaomi MiMo call is priced at xiaomi's own list price, at every
// region: the Token Plans' catalogs, which list every model at $0 and came
// first in the preset's catalog, are not read for prices.
func TestXiaomiListPrice(t *testing.T) {
	useXiaomiCatalog(t)
	oldDefault := "xiaomi-token-plan-cn, xiaomi"
	for _, c := range []struct{ region, cat string }{
		{"api", oldDefault}, {"api", "xiaomi"},
		{"plan-cn", oldDefault}, {"plan-sgp", oldDefault}, {"plan-ams", oldDefault},
		// a catalog the user named: the plan's alone, or the plan's first
		{"api", "xiaomi-token-plan-sgp"}, {"plan-ams", "openrouter, xiaomi-token-plan-ams, xiaomi"},
	} {
		p := xiaomiAt(t, c.region, c.cat)
		for model, want := range map[string]catalog.Price{"mimo-v2.6-flash": mimoFlash, "mimo-v2.6-pro": mimoPro} {
			if pr, ok := p.ListPrice(model); !ok || !pr.Same(want) {
				t.Errorf("%s at %q, %s: %+v %v, want %+v", c.region, p.Catalog, model, pr, ok, want)
			}
		}
	}
	// with the provider gone, its maker's price: not the plan's $0
	if pr, ok := MakerPrice("mimo-v2.6-flash"); !ok || !pr.Same(mimoFlash) {
		t.Errorf("MakerPrice(mimo-v2.6-flash) = %+v %v, want %+v", pr, ok, mimoFlash)
	}
	// a catalog listing the model at $0 ahead of one with its price is
	// still read first: a relay's own free model stays free
	relay := Provider{ID: "relay", Catalog: "freebie, xiaomi"}
	if pr, ok := relay.ListPrice("mimo-v2.6-flash"); !ok || !pr.Same(catalog.Price{}) {
		t.Errorf("relay mimo-v2.6-flash: %+v %v, want its own $0", pr, ok)
	}
	// a Xiaomi provider isn't a Kimi Code one: its ids aren't Kimi's
	if xiaomiAt(t, "plan-cn", oldDefault).kimiCodeMember() {
		t.Error("a Xiaomi Token Plan provider counts as a Kimi Code membership")
	}

	// what `magpie model price` and the Usage page read: the saved provider's
	p := xiaomiAt(t, "api", oldDefault)
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	if pr, ok := EffectivePrice("xiaomi", "mimo-v2.6-flash"); !ok || !pr.Same(mimoFlash) {
		t.Errorf("EffectivePrice(xiaomi, mimo-v2.6-flash) = %+v %v, want %+v", pr, ok, mimoFlash)
	}
}

// #1873: each region of Xiaomi MiMo looks its models up in its own
// catalogs, pay as you go in xiaomi's alone. A provider saved with the
// preset's catalog, or another region's, follows the region it is at; one
// the user named stays as they named it.
func TestXiaomiRegionCatalogs(t *testing.T) {
	useXiaomiCatalog(t)
	want := map[string][]string{
		"plan-cn":  {"xiaomi-token-plan-cn", "xiaomi"},
		"plan-sgp": {"xiaomi-token-plan-sgp", "xiaomi"},
		"plan-ams": {"xiaomi-token-plan-ams", "xiaomi"},
		"api":      {"xiaomi"},
	}
	pr := Preset("xiaomi")
	if len(pr.Regions) != len(want) {
		t.Fatalf("xiaomi has %d regions, the test knows %d", len(pr.Regions), len(want))
	}
	for region, cats := range want {
		// a new provider, the old default saved before this, and one
		// moved from another region in the editor
		for _, from := range []string{pr.Catalog, "xiaomi", "xiaomi-token-plan-sgp, xiaomi"} {
			if got := xiaomiAt(t, region, from).Catalogs(); !slices.Equal(got, cats) {
				t.Errorf("%s saved with %q: catalogs %v, want %v", region, from, got, cats)
			}
		}
		// added at that region from a link
		p, err := ParseImport("magpie://import?preset=xiaomi&region=" + region)
		if err != nil {
			t.Fatal(err)
		}
		if got := p.Catalogs(); !slices.Equal(got, cats) {
			t.Errorf("import at %s: catalogs %v, want %v", region, got, cats)
		}
	}
	// the preset's own endpoints are plan-cn's, and so is its catalog
	if p, _ := FromPreset("xiaomi"); !slices.Equal(p.Catalogs(), want["plan-cn"]) {
		t.Errorf("FromPreset(xiaomi) catalogs %v", p.Catalogs())
	}
	// a catalog the user named is theirs, at any region
	for _, region := range []string{"api", "plan-sgp"} {
		if got := xiaomiAt(t, region, "openrouter, xiaomi").Catalog; got != "openrouter, xiaomi" {
			t.Errorf("%s: the user's catalog became %q", region, got)
		}
	}
}
