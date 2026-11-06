package gui

import (
	"os"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// The Providers page shows a plugin model's load for a model the user
// lists by another id (Listed as) too: the plugin reports it by its own id
// (axelburks on Discord: a renamed model showed no load).
func TestModelLoadListedAsAnotherID(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	plugin.ForgetLoads()
	t.Cleanup(plugin.ForgetLoads)
	s := settings.Load()
	s.ModelIDs = map[string]string{"inhouse-plugin/fast": "big", "inhouse-plugin/grok": "grok-4.7@256k"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	plugin.NoteLoadsForTest("inhouse", map[string]float64{"big": 87.4, "grok-4.7": 60, "small": 12}, time.Now())
	p := provider.PluginForTest("inhouse-plugin", "inhouse")
	ms := []catalog.Model{{ID: "fast"}, {ID: "grok"}, {ID: "small"}, {ID: "quiet"}}
	info := providerInfoWith(p, provider.Listing{Available: ms, Exposed: ms}, nil, nil)
	want := map[string]float64{"fast": 87.4, "grok": 60, "small": 12}
	seen := 0
	for _, m := range info.Models {
		w, has := want[m.ID]
		switch {
		case !has && m.Load != nil:
			t.Errorf("%s: load %v, want none", m.ID, *m.Load)
		case has && m.Load == nil:
			t.Errorf("%s: no load, want %v", m.ID, w)
		case has && *m.Load != w:
			t.Errorf("%s: load %v, want %v", m.ID, *m.Load, w)
		}
		if has {
			seen++
		}
		if m.ID == "fast" && m.Vendor != "big" {
			t.Errorf("fast: Vendor %q, want big", m.Vendor)
		}
	}
	if seen != len(want) {
		t.Fatalf("models %+v", info.Models)
	}
}
