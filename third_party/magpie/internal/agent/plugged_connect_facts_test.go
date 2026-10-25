package agent

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// An agent plugin's connect is told, of each model in models and of
// model, what /v1/models says of it by the same names: native_endpoints,
// the APIs a request goes on untranslated, and pricing, OpenRouter's USD
// a token (cleary5779 on Discord, a self-built agent picking the API that
// passes straight through and showing what a call costs). A model with no
// known price has no pricing at all, never a 0.
func TestPluggedConnectSeesEndpointsAndPricing(t *testing.T) {
	home := pluggedHome(t, `
export const agent = { id: "facts", name: "Facts", config: "~/.facts.json", model: "model" };
export function connect({ gateway, model, models }) {
  return { "seen": JSON.stringify({ model, models }) };
}
`)
	if err := provider.SetModelPrice("deepseek/pro", &catalog.Price{Input: 0.3, Output: 1.2, CacheRead: 0.03}); err != nil {
		t.Fatal(err)
	}
	a, err := Find("facts")
	if err != nil {
		t.Fatal(err)
	}
	if err := a.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(home, ".facts.json"))
	if err != nil {
		t.Fatal(err)
	}
	var file struct{ Seen string }
	if err := json.Unmarshal(b, &file); err != nil {
		t.Fatalf("%v:\n%s", err, b)
	}
	type entry struct {
		ID        string            `json:"id"`
		Native    []string          `json:"native_endpoints"`
		Pricing   map[string]string `json:"pricing"`
		HasPrice  bool              `json:"-"`
		HasNative bool              `json:"-"`
	}
	var seen struct {
		Model  map[string]json.RawMessage   `json:"model"`
		Models []map[string]json.RawMessage `json:"models"`
	}
	if err := json.Unmarshal([]byte(file.Seen), &seen); err != nil {
		t.Fatalf("%v:\n%s", err, file.Seen)
	}
	read := func(raw map[string]json.RawMessage) entry {
		var e entry
		b, _ := json.Marshal(raw)
		json.Unmarshal(b, &e)
		_, e.HasPrice = raw["pricing"]
		_, e.HasNative = raw["native_endpoints"]
		return e
	}
	byID := map[string]entry{}
	for _, m := range seen.Models {
		e := read(m)
		byID[e.ID] = e
	}
	pro, flash, model := byID["deepseek/pro"], byID["deepseek/flash"], read(seen.Model)
	want := map[string]string{"prompt": "0.0000003", "completion": "0.0000012", "input_cache_read": "0.00000003"}
	for name, e := range map[string]entry{"models' pro": pro, "model": model} {
		if len(e.Native) != 1 || e.Native[0] != "/v1/chat/completions" {
			t.Errorf("%s native_endpoints = %v, want [/v1/chat/completions]:\n%s", name, e.Native, file.Seen)
		}
		if len(e.Pricing) != len(want) {
			t.Errorf("%s pricing = %v, want %v", name, e.Pricing, want)
		}
		for k, v := range want {
			if e.Pricing[k] != v {
				t.Errorf("%s pricing[%s] = %q, want %q", name, k, e.Pricing[k], v)
			}
		}
	}
	if flash.ID == "" || !flash.HasNative {
		t.Errorf("flash missing or without native_endpoints:\n%s", file.Seen)
	}
	if flash.HasPrice {
		t.Errorf("unpriced flash has pricing %v, want none", flash.Pricing)
	}
	// what connect was told before stays as it was
	for _, k := range []string{"id", "name", "context", "output", "images", "efforts"} {
		if _, ok := seen.Model[k]; !ok {
			t.Errorf("model lost %q:\n%s", k, file.Seen)
		}
	}
}
