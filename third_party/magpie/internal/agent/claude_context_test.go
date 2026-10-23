package agent

import (
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// TestClaudeContextWindow: Claude Code takes a model it doesn't know for a
// 200K one, so magpie tells it the window of the model it switches it to
// (CLAUDE_CODE_MAX_CONTEXT_TOKENS), changes it with the model, takes it away
// for a model it doesn't know the window of and for Claude's own, and never
// touches a value the user set (#311).
func TestClaudeContextWindow(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "https://example.test/v1", Key: "k",
		Models: []string{"small", "mid", "big", "plain"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("v", "https://example.test/v1", []catalog.Model{
		{ID: "small", Context: 128000}, {ID: "mid", Context: 400000}, {ID: "big", Context: 1000000}, {ID: "plain"},
	}); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(home, ".claude", "settings.json")
	writeFile(t, path, `{"theme":"dark"}`)
	a := claude(home)
	window := func() string { v, _ := edit.GetJSON(path, "env."+claudeContextEnv); return v }
	set := func(key, v string) {
		t.Helper()
		if err := a.Field(key).Set(v); err != nil {
			t.Fatal(err)
		}
	}

	set("model", "v/small")
	if window() != "128000" {
		t.Fatalf("small: %q", window())
	}
	set("model", "v/mid")
	if window() != "400000" {
		t.Fatalf("switched to mid: %q", window())
	}
	set("model", "v/plain")
	if window() != "" {
		t.Fatalf("a model of no known window keeps the last one's: %q", window())
	}
	// a 1M model is marked [1m], which Claude Code takes for 1M whatever
	// the window says; beside a tier that isn't, the window told is the
	// main's own, and the tier compacts by its own (多喝热水 on Discord:
	// the tier's read as the main's), under Full window and without it
	set("model", "v/big[1m]")
	if window() != "" {
		t.Fatalf("[1m]: %q", window())
	}
	budget := func(id string) string { v, _ := edit.GetJSON(path, budgetPath(id)); return v }
	compact := func() (env, top string) {
		env, _ = edit.GetJSON(path, "env."+claudeCompactEnv)
		top, _ = edit.GetJSON(path, "autoCompactWindow")
		return env, top
	}
	set("haiku", "v/small")
	if e, top := compact(); window() != "1000000" || budget("v/small") != "128000" || budget("v/big[1m]") != "" || e != "" || top != "272000" {
		t.Fatalf("haiku on small beside a [1m] model: window %q, small %q, big %q, compact env %q, top-level %q",
			window(), budget("v/small"), budget("v/big[1m]"), e, top)
	}
	if err := settings.Save(settings.Settings{FullContext: true}); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if e, top := compact(); window() != "1000000" || budget("v/small") != "128000" || e != "" || top != "" {
		t.Fatalf("full window: window %q, small %q, compact env %q, top-level %q", window(), budget("v/small"), e, top)
	}
	if err := settings.Save(settings.Settings{}); err != nil {
		t.Fatal(err)
	}
	set("haiku", "")
	if e, top := compact(); window() != "" || budget("v/small") != "" || e != "272000" || top != "" {
		t.Fatalf("the tier gone: window %q, small %q, compact env %q, top-level %q", window(), budget("v/small"), e, top)
	}
	set("model", "v/mid")
	if window() != "400000" {
		t.Fatalf("the main model's window wins: %q", window())
	}
	// back to Claude's own model, and to Claude Code as installed
	set("model", "claude-opus-4-8")
	if window() != "" {
		t.Fatalf("Claude's own model kept magpie's window: %q", window())
	}
	set("model", "v/mid")
	set("model", "")
	if window() != "" {
		t.Fatalf("reset kept magpie's window: %q", window())
	}
	if v, _ := edit.GetJSON(path, "theme"); v != "dark" {
		t.Fatal("theme lost")
	}

	// the user's own value is theirs, set before magpie or over magpie's
	if err := edit.SetJSON(path, edit.KV{Path: "env." + claudeContextEnv, Value: "64000"}); err != nil {
		t.Fatal(err)
	}
	for _, v := range []string{"v/small", "v/plain", "v/mid", "claude-opus-4-8", ""} {
		set("model", v)
		if window() != "64000" {
			t.Fatalf("the user's value after %q: %q", v, window())
		}
	}
	if err := edit.DelJSON(path, "env."+claudeContextEnv); err != nil {
		t.Fatal(err)
	}
	set("model", "v/small")
	if err := edit.SetJSON(path, edit.KV{Path: "env." + claudeContextEnv, Value: "100000"}); err != nil {
		t.Fatal(err)
	}
	set("model", "v/mid")
	set("model", "")
	if window() != "100000" {
		t.Fatalf("a value the user changed magpie's to: %q", window())
	}
	// and one the same as magpie would write, but not written by it
	set("model", "v/small")
	if window() != "100000" {
		t.Fatalf("user value replaced: %q", window())
	}
	if err := edit.SetJSON(path, edit.KV{Path: "env." + claudeContextEnv, Value: "400000"}); err != nil {
		t.Fatal(err)
	}
	set("model", "")
	if window() != "400000" {
		t.Fatalf("the user's 400000 was taken for magpie's: %q", window())
	}
}
