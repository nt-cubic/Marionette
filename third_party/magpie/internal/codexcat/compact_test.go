package codexcat

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// Codex is told the threshold the user set (#876, hisiling: 272K was the
// only one): a model's own or its provider's before the one for every
// model, which may be any size too; one at or above the window is the
// whole window, and Full window still runs the rest to theirs.
func TestCodexCatalogCompactAt(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(os.Getenv("HOME"), ".config"))
	windows := func() map[string][2]any {
		var got struct {
			Models []map[string]any `json:"models"`
		}
		json.Unmarshal(Catalog([]catalog.Model{
			{ID: "d/flash", Name: "flash", Context: 1000000, Compact: 500000},
			{ID: "d/pro", Name: "pro", Context: 1000000, Compact: 2000000},
			{ID: "x/big", Name: "big", Context: 1000000},
			{ID: "x/small", Name: "small", Context: 128000},
		}), &got)
		out := map[string][2]any{}
		for _, e := range got.Models {
			if e["max_context_window"] != nil {
				t.Errorf("%s: max_context_window %v; the window is context_window", e["slug"], e["max_context_window"])
			}
			out[e["slug"].(string)] = [2]any{e["context_window"], e["auto_compact_token_limit"]}
		}
		return out
	}
	check := func(what string, want map[string][2]any) {
		t.Helper()
		got := windows()
		for id, w := range want {
			if got[id] != w {
				t.Errorf("%s: %s told %v; want %v", what, id, got[id], w)
			}
		}
	}
	check("default", map[string][2]any{
		"d/flash": {float64(1000000), float64(500000)},
		"d/pro":   {float64(1000000), nil},
		"x/big":   {float64(1000000), float64(settings.WorkingWindow)},
		"x/small": {float64(128000), nil},
	})
	if err := settings.Save(settings.Settings{CompactAt: 400000}); err != nil {
		t.Fatal(err)
	}
	check("compact at 400K", map[string][2]any{
		"d/flash": {float64(1000000), float64(500000)},
		"x/big":   {float64(1000000), float64(400000)},
		"x/small": {float64(128000), nil},
	})
	if err := settings.Save(settings.Settings{CompactAt: 400000, FullContext: true}); err != nil {
		t.Fatal(err)
	}
	check("full window", map[string][2]any{
		"d/flash": {float64(1000000), float64(500000)},
		"x/big":   {float64(1000000), nil},
	})
}

// Codex shows a model's context_window (95% of it) as the conversation's
// context, so a 1M model of a plugin's told magpie's 272K threshold there
// showed 258K (01huadalang on Discord, Command Code's 1M models). The
// window is the model's own, and the threshold goes where Codex compacts
// (auto_compact_token_limit), for a model magpie describes and for a
// relay's own entry alike.
func TestCodexWindowShownCompactApart(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(os.Getenv("HOME"), ".config"))
	relay := json.RawMessage(`{"slug":"big-listed","display_name":"Big","base_instructions":"x","context_window":1000000,"max_context_window":1000000,"auto_compact_token_limit":null,"supported_reasoning_levels":[]}`)
	entries := func() map[string]map[string]any {
		var got struct {
			Models []map[string]any `json:"models"`
		}
		json.Unmarshal(Catalog([]catalog.Model{
			{ID: "commandcode/claude-opus-5-5", Name: "Claude Opus 5.5", Context: 1000000},
			{ID: "relay/big-listed", Name: "Big", Context: 1000000, Codex: relay},
			{ID: "relay/big-own", Name: "Big own", Context: 1000000, Codex: relay, Compact: 200000},
		}), &got)
		out := map[string]map[string]any{}
		for _, e := range got.Models {
			out[e["slug"].(string)] = e
		}
		return out
	}
	check := func(what string, want map[string][2]any) {
		t.Helper()
		got := entries()
		for id, w := range want {
			e := got[id]
			if e == nil {
				t.Fatalf("%s: no entry for %s", what, id)
			}
			if have := [2]any{e["context_window"], e["auto_compact_token_limit"]}; have != w {
				t.Errorf("%s: %s told window/compact %v; want %v", what, id, have, w)
			}
		}
	}
	// magpie's default: the plugin's 1M model shows 1M and compacts at
	// 272K; a relay's entry is the vendor's (its null), unless the user
	// set a threshold on it
	check("default", map[string][2]any{
		"commandcode/claude-opus-5-5": {float64(1000000), float64(settings.WorkingWindow)},
		"relay/big-listed":            {float64(1000000), nil},
		"relay/big-own":               {float64(1000000), float64(200000)},
	})
	if err := settings.Save(settings.Settings{CompactAt: 400000}); err != nil {
		t.Fatal(err)
	}
	check("compact at 400K", map[string][2]any{
		"commandcode/claude-opus-5-5": {float64(1000000), float64(400000)},
		"relay/big-listed":            {float64(1000000), float64(400000)},
		"relay/big-own":               {float64(1000000), float64(200000)},
	})
	if err := settings.Save(settings.Settings{FullContext: true}); err != nil {
		t.Fatal(err)
	}
	check("full window", map[string][2]any{
		"commandcode/claude-opus-5-5": {float64(1000000), nil},
		"relay/big-listed":            {float64(1000000), nil},
	})
}
