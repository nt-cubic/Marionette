package agent

import (
	"encoding/json"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
)

// A window the user typed is the one Codex and Claude Code run to (#1809,
// a DeepSeek V4.1 Flash group set to 500K): a routing group's own size, or
// one set on the provider for a model, isn't cut to magpie's default
// working window (272K, which Codex ran to 95% of: ~258K). Codex is told
// it as its context_window, on disk and in the gateway's list, with no
// max_context_window; Claude Code is handed no CLAUDE_CODE_AUTO_COMPACT_
// WINDOW, so it runs to the CLAUDE_CODE_MAX_CONTEXT_TOKENS it is told. A
// model whose window nobody typed keeps the default, and a size typed in
// Settings (Compact at) still comes before a typed window.
func TestTypedWindowReachesCodexAndClaudeCode(t *testing.T) {
	home := syncHome(t)
	catalog.Changed = SyncCatalog
	t.Cleanup(func() { catalog.Changed = nil })
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	must(provider.Save(provider.Provider{ID: "ds", Name: "DS", Key: "k", Chat: "http://127.0.0.1:1/v1",
		Models: []string{"flash", "pro"}, Contexts: map[string]int{"pro": 400000}}))
	must(catalog.SaveLive("ds", "http://127.0.0.1:1/v1", []catalog.Model{{ID: "flash", Context: 1000000}, {ID: "pro", Context: 1000000}}))
	must(provider.SaveGroup(provider.Group{ID: "flash", Name: "Flash", Members: []string{"ds/flash"}, Context: 500000}))

	must(codex(home).Fields[0].Set("group/flash"))
	type window struct{ context, max int }
	onDisk := func() map[string]window {
		t.Helper()
		var got struct {
			Models []struct {
				Slug    string `json:"slug"`
				Context int    `json:"context_window"`
				Max     int    `json:"max_context_window"`
			} `json:"models"`
		}
		raw := readFile(filepath.Join(home, ".codex", "magpie-models.json"))
		if err := json.Unmarshal([]byte(raw), &got); err != nil {
			t.Fatalf("%v: %s", err, raw)
		}
		out := map[string]window{}
		for _, m := range got.Models {
			out[m.Slug] = window{m.Context, m.Max}
		}
		return out
	}
	listed := func() map[string]int {
		out := map[string]int{}
		for _, m := range provider.CodexListed() {
			out[m.ID] = m.Compact
		}
		return out
	}
	expect := func(what string, want map[string]window, compacts map[string]int) {
		t.Helper()
		got := onDisk()
		for slug, w := range want {
			if got[slug] != w {
				t.Errorf("%s: on disk %s is told %+v; want %+v", what, slug, got[slug], w)
			}
		}
		l := listed()
		for id, n := range compacts {
			if l[id] != n {
				t.Errorf("%s: the gateway's list compacts %s at %d; want %d", what, id, l[id], n)
			}
		}
	}
	expect("typed windows", map[string]window{
		"group/flash": {500000, 0},       // the group's own size
		"ds/pro":      {400000, 0},       // the provider's for the model
		"ds/flash":    {272000, 1000000}, // nobody typed it: the default
	}, map[string]int{"group/flash": 500000, "ds/pro": 400000, "ds/flash": 0})

	path := filepath.Join(home, ".claude", "settings.json")
	writeFile(t, path, `{}`)
	a := claude(home)
	env := func(k string) string { v, _ := edit.GetJSON(path, "env."+k); return v }
	must(a.Field("model").Set("group/flash"))
	if c, w := env(claudeCompactEnv), env(claudeContextEnv); c != "" || w != "500000" {
		t.Errorf("Claude Code on the group: compact %q, window %q; want none and 500000", c, w)
	}
	must(a.Field("model").Set("ds/pro"))
	if c, w := env(claudeCompactEnv), env(claudeContextEnv); c != "" || w != "400000" {
		t.Errorf("Claude Code on ds/pro: compact %q, window %q; want none and 400000", c, w)
	}
	must(a.Field("model").Set("ds/flash"))
	if c := env(claudeCompactEnv); c != "272000" {
		t.Errorf("Claude Code on ds/flash: compact %q; want the default 272000", c)
	}

	// a size the user typed in Settings comes first
	must(provider.SetCompactAt(300000))
	SyncCatalog()
	expect("Compact at 300K", map[string]window{
		"group/flash": {300000, 500000},
		"ds/pro":      {300000, 400000},
	}, map[string]int{"group/flash": 0, "ds/pro": 0})
	must(a.Field("model").Set("group/flash"))
	if c := env(claudeCompactEnv); c != "300000" {
		t.Errorf("Claude Code on the group with Compact at 300K: %q", c)
	}
}

// #2224 (Zypixh): a group that follows its smallest member is told that
// member's window, and when the user typed it (500K on the provider for
// gpt-6.1-sol) Claude Desktop and Codex run to it whole, as they do for a
// group set to 500K itself, not to magpie's default 272K. A group in a
// group passes it on. A group whose smallest member's window nobody typed
// keeps the default.
func TestSmallestMemberTypedWindow(t *testing.T) {
	home, path := desktopWindowSandbox(t)
	must := func(err error) {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
	}
	must(provider.Save(provider.Provider{ID: "w", Name: "W", Key: "k", Chat: "http://127.0.0.1:1/v1",
		Models: []string{"sol", "other"}, Contexts: map[string]int{"sol": 500000}}))
	must(catalog.SaveLive("w", "http://127.0.0.1:1/v1", []catalog.Model{{ID: "sol", Context: 1000000}, {ID: "other", Context: 600000}}))
	must(provider.SaveGroup(provider.Group{ID: "pool", Name: "Pool", Members: []string{"w/sol", "v/glm"}, Context: provider.ContextSmallest}))
	must(provider.SaveGroup(provider.Group{ID: "outer", Name: "Outer", Members: []string{"group/pool", "v/glm"}, Context: provider.ContextSmallest}))
	must(provider.SaveGroup(provider.Group{ID: "plain", Name: "Plain", Members: []string{"w/other", "v/glm"}, Context: provider.ContextSmallest}))

	// Codex: the gateway's list and its context_window
	must(codex(home).Fields[0].Set("group/pool"))
	compacts := map[string]int{}
	for _, m := range provider.CodexListed() {
		compacts[m.ID] = m.Compact
	}
	for id, want := range map[string]int{"group/pool": 500000, "group/outer": 500000, "group/plain": 0} {
		if compacts[id] != want {
			t.Errorf("Codex: the gateway's list compacts %s at %d; want %d", id, compacts[id], want)
		}
	}

	// Claude Desktop: each group's own autoCompactWindow
	must(claudeDesktop(home).Field("provider").Set("magpie"))
	ids := map[string]string{}
	for _, e := range provider.Catalog() {
		ids[e.ID] = gateway.DesktopID(e)
	}
	for id, want := range map[string]string{"group/pool": "500000", "group/outer": "500000", "group/plain": "272000"} {
		if v, _ := edit.GetJSON(path, budgetPath(ids[id])); v != want {
			t.Errorf("Claude Desktop: %s (%s) compacts at %q; want %q", id, ids[id], v, want)
		}
	}
}
