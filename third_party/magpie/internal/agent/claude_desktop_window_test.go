package agent

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// desktopWindowSandbox is a HOME with provider v's models: gpt 272K and
// grok 256K (reasoning, so Desktop lists them by mythos-magpie ids), glm
// 1M, plain of no known window, and a Claude-named one Claude Code knows.
func desktopWindowSandbox(t *testing.T) (home, path string) {
	home = t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("APPDATA", filepath.Join(home, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(home, "AppData", "Local"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k",
		Models: []string{"gpt", "grok", "glm", "plain", "claude-opus-4-6"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("v", "http://127.0.0.1:1/v1", []catalog.Model{
		{ID: "gpt", Context: 272000}, {ID: "grok", Context: 256000}, {ID: "glm", Context: 1000000},
		{ID: "plain"}, {ID: "claude-opus-4-6", Context: 128000},
	}); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"v/gpt", "v/grok", "v/claude-opus-4-6"} {
		if err := provider.SetModelEfforts(id, []string{"low", "medium", "high"}); err != nil {
			t.Fatal(err)
		}
	}
	path = filepath.Join(home, ".claude", "settings.json")
	writeFile(t, path, `{"theme":"dark"}`)
	return home, path
}

// TestClaudeDesktopContextWindow (#1458, #2025): Claude Desktop's Code tab
// runs Claude Code on ~/.claude/settings.json with the id Desktop lists a
// model by, and hands it nothing of the model's window; Claude Code 2.1.293
// takes a mythos-magpie id it doesn't know for 200K unless
// CLAUDE_CODE_MAX_CONTEXT_TOKENS says otherwise, so GPT 272K and Grok 256K
// showed "/ 200K". That variable is one value for all of them, and the
// least held a 1M model to 256K (#2025, 6.1sol), so while Desktop is on
// magpie it is the largest window of Desktop's and Claude Code's models,
// and each model below it runs at its own through
// modelSettings.<id>.autoCompactWindow. A model Claude Code knows, and one
// listed by its 1M id, are left out of the largest; the user's own value
// is theirs.
func TestClaudeDesktopContextWindow(t *testing.T) {
	home, path := desktopWindowSandbox(t)
	a := claude(home)
	window := func() string { v, _ := edit.GetJSON(path, "env."+claudeContextEnv); return v }
	set := func(key, v string) {
		t.Helper()
		if err := a.Field(key).Set(v); err != nil {
			t.Fatal(err)
		}
	}
	desktop := func(v string) {
		t.Helper()
		if err := claudeDesktop(home).Field("provider").Set(v); err != nil {
			t.Fatal(err)
		}
	}
	ids := map[string]string{}
	for _, e := range provider.Catalog() {
		ids[e.ID] = gateway.DesktopID(e)
	}
	// the ids Desktop's Code tab hands Claude Code are ones it doesn't know
	for _, id := range []string{"v/gpt", "v/grok"} {
		if !strings.HasPrefix(ids[id], "mythos-magpie-") || claudeName(ids[id]) != "" {
			t.Fatalf("%s is listed as %s", id, ids[id])
		}
	}
	budget := func(ref string) string {
		v, _ := edit.GetJSON(path, budgetPath(ids[ref]))
		return v
	}

	// Claude Code on magpie on a model of no known window: nothing to
	// tell it until Desktop is on magpie
	set("model", "v/plain")
	if window() != "" {
		t.Fatalf("plain, Desktop off: %q", window())
	}
	desktop("magpie")
	// glm's 1M is the largest; GPT is held to the working window (272K)
	// as the top-level autoCompactWindow holds every model, Grok to its
	// own 256K
	if window() != "1000000" {
		t.Fatalf("Desktop on magpie, Claude Code on plain: %q, want glm's 1000000", window())
	}
	if v, _ := edit.GetJSON(path, "autoCompactWindow"); v != "272000" {
		t.Fatalf("top-level autoCompactWindow %q, want the working window", v)
	}
	if v, _ := edit.GetJSON(path, "env."+claudeCompactEnv); v != "" {
		t.Fatalf("%s %q would take precedence over every model's own", claudeCompactEnv, v)
	}
	if budget("v/grok") != "256000" || budget("v/gpt") != "" || budget("v/glm") != "" {
		t.Fatalf("grok %q gpt %q glm %q: %s", budget("v/grok"), budget("v/gpt"), budget("v/glm"), readFile(path))
	}
	// Full window: each its own whole
	s := settings.Load()
	s.FullContext = true
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if v, _ := edit.GetJSON(path, "autoCompactWindow"); v != "" || window() != "1000000" || budget("v/grok") != "256000" || budget("v/gpt") != "272000" || budget("v/glm") != "" {
		t.Fatalf("Full window: top %q window %q grok %q gpt %q glm %q", v, window(), budget("v/grok"), budget("v/gpt"), budget("v/glm"))
	}
	s.FullContext = false
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	// Claude Code's own model of 272K gets its own too
	set("model", "v/gpt")
	if window() != "1000000" || budget("v/grok") != "256000" {
		t.Fatalf("main gpt beside Desktop's: %q %q", window(), budget("v/grok"))
	}
	// a model added later reaches it with the rest of the catalog
	if err := provider.Save(provider.Provider{ID: "w", Name: "W", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"small.v2"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("w", "http://127.0.0.1:1/v1", []catalog.Model{{ID: "small.v2", Context: 128000}}); err != nil {
		t.Fatal(err)
	}
	for _, e := range provider.Catalog() {
		ids[e.ID] = gateway.DesktopID(e)
	}
	if err := a.Sync(); err != nil || window() != "1000000" || budget("w/small.v2") != "128000" {
		t.Fatalf("after w/small.v2 was added: %q %q %v: %s", window(), budget("w/small.v2"), err, readFile(path))
	}
	if err := provider.Delete("w"); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil || budget("w/small.v2") != "" || !strings.Contains(readFile(path), ids["v/grok"]) {
		t.Fatalf("after w/small.v2 went: %v %s", err, readFile(path))
	}
	// the user's own entry for a model, and a window of their own past the
	// model's, which holds the largest to it
	if err := edit.SetJSON(path, edit.KV{Path: budgetPath(ids["v/grok"]), Value: "auto"}); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil || window() != "256000" || budget("v/grok") != "auto" {
		t.Fatalf("the user's grok entry: window %q grok %q %v", window(), budget("v/grok"), err)
	}
	if err := edit.SetJSON(path, edit.KV{Path: budgetPath(ids["v/grok"]), Value: 100000}); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil || window() != "1000000" || budget("v/grok") != "100000" {
		t.Fatalf("the user's grok 100000: window %q grok %q %v", window(), budget("v/grok"), err)
	}
	if err := edit.DelJSON(path, "modelSettings"); err != nil {
		t.Fatal(err)
	}
	// the user's own top-level autoCompactWindow is theirs, and the one a
	// model without an entry runs at
	if err := edit.SetJSON(path, edit.KV{Path: "autoCompactWindow", Value: 500000}); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if v, _ := edit.GetJSON(path, "autoCompactWindow"); v != "500000" || window() != "1000000" || budget("v/grok") != "256000" || budget("v/gpt") != "272000" || budget("v/glm") != "" {
		t.Fatalf("the user's top-level: %q window %q grok %q gpt %q glm %q", v, window(), budget("v/grok"), budget("v/gpt"), budget("v/glm"))
	}
	// Desktop off: Claude Code's own model's window again, magpie's
	// entries gone, the user's kept
	if err := edit.SetJSON(path, edit.KV{Path: "modelSettings.mine.autoCompactWindow", Value: 90000}); err != nil {
		t.Fatal(err)
	}
	desktop("")
	if window() != "272000" || budget("v/grok") != "" || budget("v/gpt") != "" {
		t.Fatalf("Desktop off, main gpt: %q: %s", window(), readFile(path))
	}
	if v, _ := edit.GetJSON(path, "autoCompactWindow"); v != "500000" {
		t.Fatalf("the user's top-level went: %s", readFile(path))
	}
	if v, _ := edit.GetJSON(path, "modelSettings.mine.autoCompactWindow"); v != "90000" {
		t.Fatalf("the user's entry went: %s", readFile(path))
	}
	set("model", "claude-opus-4-8")
	if window() != "" {
		t.Fatalf("Desktop off, Claude's own model: %q", window())
	}

	// the user's own value is theirs, through Desktop, a switch and a Sync
	if err := edit.SetJSON(path, edit.KV{Path: "env." + claudeContextEnv, Value: "64000"}); err != nil {
		t.Fatal(err)
	}
	desktop("magpie")
	set("model", "v/gpt")
	if err := a.Sync(); err != nil || window() != "64000" || budget("v/grok") != "" {
		t.Fatalf("the user's value was overwritten: %q %v %s", window(), err, readFile(path))
	}
	set("model", "")
	if window() != "64000" {
		t.Fatalf("the user's value was taken away: %q", window())
	}
	if v, _ := edit.GetJSON(path, "theme"); v != "dark" {
		t.Fatal("theme lost")
	}
}

// Desktop's compatibility aliases must not send Explore to the Opus tier.
// The setting is shared with Claude Code, but only owned while Desktop is
// on magpie; an existing user value, including an empty one, stays theirs.
func TestClaudeDesktopExploreInheritCap(t *testing.T) {
	const key = "env.CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP"
	for _, routed := range []bool{false, true} {
		name := "desktop-alone"
		if routed {
			name = "beside-cli"
		}
		t.Run(name, func(t *testing.T) {
			home, path := desktopWindowSandbox(t)
			a := claude(home)
			must := func(err error) {
				t.Helper()
				if err != nil {
					t.Fatal(err)
				}
			}
			check := func(want string, present bool) {
				t.Helper()
				got, has := edit.GetJSON(path, key)
				if got != want || has != present {
					t.Fatalf("Explore inherit cap: value %q, present %v; want %q, %v", got, has, want, present)
				}
			}
			if routed {
				must(a.Field("model").Set("v/gpt"))
				must(a.Field("model").Set("v/grok"))
			}
			must(a.Sync())
			check("", false)
			d := claudeDesktop(home)
			must(d.Field("provider").Set("magpie"))
			check("1", true)
			must(a.Sync())
			check("1", true)
			// A model pick goes through writeTiers, not the regular Sync.
			must(a.Field("model").Set("v/gpt"))
			check("1", true)
			if !routed {
				must(a.Unwire())
				check("1", true)
				must(a.Sync())
				check("1", true)
			}
			must(d.Field("provider").Set(""))
			check("", false)
			must(a.Sync())
			check("", false)
			// Reconnecting must acquire and release ownership again.
			must(d.Field("provider").Set("magpie"))
			check("1", true)
			must(a.Field("model").Set(""))
			check("1", true)
			must(d.Field("provider").Set(""))
			check("", false)
		})
	}
}

func TestClaudeDesktopExploreKeepsUserValue(t *testing.T) {
	const key = "env.CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP"
	for _, mine := range []string{"0", "1", ""} {
		for _, changed := range []bool{false, true} {
			// Writing the same value after magpie cannot be distinguished
			// from leaving its value alone; test an existing user's 1 instead.
			if changed && mine == "1" {
				continue
			}
			name := "existing-" + mine
			if changed {
				name = "changed-" + mine
			}
			t.Run(name, func(t *testing.T) {
				home, path := desktopWindowSandbox(t)
				a, d := claude(home), claudeDesktop(home)
				must := func(err error) {
					t.Helper()
					if err != nil {
						t.Fatal(err)
					}
				}
				check := func() {
					t.Helper()
					if got, has := edit.GetJSON(path, key); !has || got != mine {
						t.Fatalf("user's Explore value %q lost: %q, present %v", mine, got, has)
					}
				}
				if changed {
					must(d.Field("provider").Set("magpie"))
					if got, has := edit.GetJSON(path, key); !has || got != "1" {
						t.Fatalf("Desktop did not install Explore compatibility: %q, %v", got, has)
					}
				}
				must(edit.SetJSON(path, edit.KV{Path: key, Value: mine}))
				must(d.Field("provider").Set("magpie"))
				check()
				must(a.Field("model").Set("v/gpt"))
				check()
				must(a.Sync())
				check()
				must(d.Field("provider").Set(""))
				check()
				must(a.Unwire())
				check()
			})
		}
	}
}

// TestClaudeDesktopContextWindow1M: a model Desktop lists by its 1M id
// (settings.DesktopLongest) runs at 1M whatever the value says, so it adds
// nothing to it; listed plain too, it runs at the value, its own 1M when it
// is the only one.
func TestClaudeDesktopContextWindow1M(t *testing.T) {
	home, path := desktopWindowSandbox(t)
	// Desktop is shown glm alone (the Agents page's model list)
	if err := provider.SetHiddenModels("claude-desktop", []string{"v/gpt", "v/grok", "v/plain", "v/claude-opus-4-6"}); err != nil {
		t.Fatal(err)
	}
	a := claude(home)
	window := func() string { v, _ := edit.GetJSON(path, "env."+claudeContextEnv); return v }
	if err := a.Field("model").Set("v/plain"); err != nil {
		t.Fatal(err)
	}
	if err := claudeDesktop(home).Field("provider").Set("magpie"); err != nil {
		t.Fatal(err)
	}
	if window() != "1000000" {
		t.Fatalf("glm listed plain: %q", window())
	}
	s := settings.Load()
	s.DesktopLongest = true
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil || window() != "" {
		t.Fatalf("glm listed by its 1M id alone: %q %v", window(), err)
	}
}

// TestClaudeDesktopAloneContextWindow (#1458, marsxxl on v0.1.1154): with
// Claude Desktop on magpie and Claude Code itself left on its own setup,
// Desktop's Code tab still runs Claude Code on ~/.claude/settings.json, so
// it is told Desktop's window (and what Desktop's models can do) all the
// same. Only wiring Claude Code in wrote them before, and its Sync did
// nothing while it wasn't, so such a user saw 200K for every model. The
// user's own Claude Code setup and settings are left as they are, and
// Desktop off takes magpie's values out again.
func TestClaudeDesktopAloneContextWindow(t *testing.T) {
	home, path := desktopWindowSandbox(t)
	writeFile(t, path, `{"theme":"dark","model":"opus","autoCompactWindow":300000}`)
	a := claude(home)
	get := func(key string) string { v, _ := edit.GetJSON(path, key); return v }
	window := func() string { return get("env." + claudeContextEnv) }
	desktop := func(v string) {
		t.Helper()
		if err := claudeDesktop(home).Field("provider").Set(v); err != nil {
			t.Fatal(err)
		}
	}

	desktop("magpie")
	if window() != "1000000" {
		t.Fatalf("Desktop alone on magpie: window %q, want glm's 1000000", window())
	}
	if caps := get("env." + claudeCapsEnv); !strings.Contains(caps, "mythos-magpie-") {
		t.Fatalf("Desktop alone on magpie: capabilities %q name none of Desktop's ids", caps)
	}
	if get("env.ANTHROPIC_BASE_URL") != "" || get("model") != "opus" || get("autoCompactWindow") != "300000" || get("theme") != "dark" {
		t.Fatalf("the user's own Claude Code setup changed: %s", readFile(path))
	}
	// the catalog's round keeps it
	if err := a.Sync(); err != nil || window() != "1000000" {
		t.Fatalf("after Sync: %q %v", window(), err)
	}
	// Claude Code wired in and out again while Desktop stays on
	if err := a.Field("model").Set("v/gpt"); err != nil {
		t.Fatal(err)
	}
	if err := a.Field("model").Set(""); err != nil {
		t.Fatal(err)
	}
	if get("env.ANTHROPIC_BASE_URL") != "" {
		t.Fatalf("Claude Code still on magpie: %s", readFile(path))
	}
	if window() != "1000000" {
		t.Fatalf("Claude Code taken off magpie, Desktop still on: %q", window())
	}
	// Desktop off: magpie's values go
	desktop("")
	if window() != "" || get("env."+claudeCapsEnv) != "" || get("modelSettings") != "" {
		t.Fatalf("Desktop off: %s", readFile(path))
	}
	if get("theme") != "dark" || get("autoCompactWindow") != "300000" {
		t.Fatalf("the user's settings changed: %s", readFile(path))
	}
}
