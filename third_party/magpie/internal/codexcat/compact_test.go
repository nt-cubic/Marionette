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
			out[e["slug"].(string)] = [2]any{e["context_window"], e["max_context_window"]}
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
		"d/flash": {float64(500000), float64(1000000)},
		"d/pro":   {float64(1000000), nil},
		"x/big":   {float64(settings.WorkingWindow), float64(1000000)},
		"x/small": {float64(128000), nil},
	})
	if err := settings.Save(settings.Settings{CompactAt: 400000}); err != nil {
		t.Fatal(err)
	}
	check("compact at 400K", map[string][2]any{
		"d/flash": {float64(500000), float64(1000000)},
		"x/big":   {float64(400000), float64(1000000)},
		"x/small": {float64(128000), nil},
	})
	if err := settings.Save(settings.Settings{CompactAt: 400000, FullContext: true}); err != nil {
		t.Fatal(err)
	}
	check("full window", map[string][2]any{
		"d/flash": {float64(500000), float64(1000000)},
		"x/big":   {float64(1000000), nil},
	})
}
