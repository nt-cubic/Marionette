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

// A model listed with a 512-token window (H20 on Discord: hy3 and Cline's
// free models) isn't told to agents at 512: Claude Code and Codex took it as
// the window and compacted at once. models.dev's window for the model
// answers, else none (the agent's default). A small window that is real
// (4096) stands, and so does one the user sets.
func TestTinyListedWindowIsUnknown(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "")
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	// models.dev's rows as it has them (2026-10-12): hy3's input under its
	// context, mimo's context
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "tencent-tokenhub":{"id":"tencent-tokenhub","models":{"hy3":{"id":"hy3","limit":{"context":256000,"input":192000,"output":128000}}}},
	  "xiaomi":{"id":"xiaomi","models":{"mimo-v2.6-flash":{"id":"mimo-v2.6-flash","limit":{"context":1048576,"output":131072}}}}}`), 0o644)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	spec := "@magpie-community/opencode-cline-auth"
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		os.MkdirAll(settings.Dir(), 0o700)
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": spec}}})
	write("plugin-auth.json", map[string]map[string]any{"cline": {"type": "api", "key": "sk-cline-1234"}})
	plugin.UseCached([]plugin.Provider{{ID: "cline", Spec: spec, Name: "Cline",
		Methods:  []plugin.Method{{Type: "api", Label: "API key"}},
		Accounts: []plugin.Account{{Key: "cline", Type: "api"}},
		Models: []plugin.Model{
			// the literal 512 as listed
			{ID: "cline-free/mimo-v2.6-flash", Name: "MiMo V2.6 Flash (free)", Free: true, Context: 512},
			{ID: "cline-free/hy3", Name: "Hy3 (free)", Free: true, Input: 512, Context: 512},
			// a prompt cap of 512 under a real window: the window answers
			{ID: "cline-free/hy3-x", Name: "Hy3-X (free)", Free: true, Input: 512, Context: 192_000},
			// nothing knows it: no window, not 512
			{ID: "cline-free/unheard-of", Name: "Unheard Of", Free: true, Context: 512},
			// a real small window stands
			{ID: "cline-pass/mythomax-l2-13b", Name: "MythoMax", Context: 4096},
		}}})
	t.Cleanup(func() { plugin.UseCached(nil) })

	for id, want := range map[string]int{
		"cline/cline-free/mimo-v2.6-flash": 1_048_576,
		"cline/cline-free/hy3":             192_000,
		"cline/cline-free/hy3-x":           192_000,
		"cline/cline-free/unheard-of":      0,
		"cline/cline-pass/mythomax-l2-13b": 4096,
	} {
		if got := pluginWindow(t, id); got != want {
			t.Errorf("%s: %d, want %d", id, got, want)
		}
	}

	// a built-in's list (WorkBuddy's /v3/config maxInputTokens) the same
	for _, tc := range []struct {
		m    catalog.Model
		want int
	}{
		{catalog.Model{ID: "hy3", Context: 512}, 192_000},
		{catalog.Model{ID: "unheard-of", Context: 512}, 0},
		{catalog.Model{ID: "unheard-of", Context: 4096}, 4096},
		{catalog.Model{ID: "hy3", Context: 200_000}, 200_000},
	} {
		if got := ListedWindow(tc.m); got != tc.want {
			t.Errorf("ListedWindow(%s at %d): %d, want %d", tc.m.ID, tc.m.Context, got, tc.want)
		}
	}

	// the user's own window is theirs, however small
	p, err := Find("cline")
	if err != nil {
		t.Fatal(err)
	}
	if err := SetContext(*p, "cline-free/unheard-of", 1024); err != nil {
		t.Fatal(err)
	}
	if got := pluginWindow(t, "cline/cline-free/unheard-of"); got != 1024 {
		t.Errorf("set by the user: %d, want 1024", got)
	}
}
