package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func tuneSandbox(t *testing.T) string {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("APPDATA", filepath.Join(home, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(home, "AppData", "Local"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	for _, k := range []string{"CLAUDE_CODE_AUTO_COMPACT_WINDOW", "FORCE_PROMPT_CACHING_5M", "CLAUDE_CODE_PROMPT_CACHE_TTL", "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL"} {
		t.Setenv(k, "")
	}
	return home
}

func tuneWrite(t *testing.T, path, s string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(s), 0o644); err != nil {
		t.Fatal(err)
	}
}

func tuneFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

// TestTuneClaudeCompact: the advised window goes where /autocompact keeps
// it, under modelSettings for the model, the user's other settings kept;
// Undo takes it out again, empty entries with it.
func TestTuneClaudeCompact(t *testing.T) {
	home := tuneSandbox(t)
	path := filepath.Join(home, ".claude", "settings.json")
	tuneWrite(t, path, `{"theme": "dark", "autoCompactWindow": 300000}`)
	s, err := TuneRead(home, "claude", TuneCompact, "claude-opus-5-5")
	if err != nil || s.Value != "300000" || s.Source != "settings" || s.Ours {
		t.Fatalf("read %+v %v: want the top-level 300000", s, err)
	}
	if err := TuneApply(home, "claude", TuneCompact, "claude-opus-5-5", "160000"); err != nil {
		t.Fatal(err)
	}
	got := strings.Join(strings.Fields(tuneFile(t, path)), "")
	if !strings.Contains(got, `"claude-opus-5-5":{"autoCompactWindow":160000}`) || !strings.Contains(got, `"theme":"dark"`) || !strings.Contains(got, `"autoCompactWindow":300000`) {
		t.Fatalf("settings.json after apply:\n%s", got)
	}
	s, _ = TuneRead(home, "claude", TuneCompact, "claude-opus-5-5")
	if s.Value != "160000" || s.Source != "model" || !s.Ours {
		t.Fatalf("read after apply %+v", s)
	}
	if err := TuneUndo(home, "claude", TuneCompact, "claude-opus-5-5"); err != nil {
		t.Fatal(err)
	}
	got = tuneFile(t, path)
	if strings.Contains(got, "modelSettings") || !strings.Contains(got, "300000") {
		t.Fatalf("settings.json after undo:\n%s", got)
	}
	if err := TuneApply(home, "claude", TuneCompact, "claude-opus-5-5", "50000"); err == nil {
		t.Fatal("took a window under Claude Code's 100000")
	}
	if err := TuneApply(home, "claude", TuneCompact, "gpt-5.5", "160000"); err == nil {
		t.Fatal("took a window for a model that is no Claude model")
	}
}

// TestTuneUserValueKept: a TTL the user had is put back by Undo, and one
// they changed after Apply is theirs: Undo leaves it.
func TestTuneUserValueKept(t *testing.T) {
	home := tuneSandbox(t)
	path := filepath.Join(home, ".claude", "settings.json")
	tuneWrite(t, path, `{"promptCacheTtl": "5m"}`)
	if err := TuneApply(home, "claude", TuneCache, "", "1h"); err != nil {
		t.Fatal(err)
	}
	if err := TuneUndo(home, "claude", TuneCache, ""); err != nil {
		t.Fatal(err)
	}
	if v, _ := TuneRead(home, "claude", TuneCache, ""); v.Value != "5m" || v.Ours {
		t.Fatalf("after undo %+v, want the user's 5m", v)
	}
	if err := TuneApply(home, "claude", TuneCache, "", "1h"); err != nil {
		t.Fatal(err)
	}
	tuneWrite(t, path, `{"promptCacheTtl": "5m", "other": 1}`)
	if err := TuneUndo(home, "claude", TuneCache, ""); err != nil {
		t.Fatal(err)
	}
	if got := tuneFile(t, path); !strings.Contains(got, `"5m"`) {
		t.Fatalf("undo wrote over the user's change:\n%s", got)
	}
	if err := TuneUndo(home, "claude", TuneCache, ""); err == nil {
		t.Fatal("undo with nothing of magpie's to put back")
	}
	if err := TuneApply(home, "claude", TuneCache, "", "2h"); err == nil {
		t.Fatal("took a lifetime Claude Code has no use for")
	}
}

// TestTuneLockedByEnv: a variable that comes before the setting is told
// of, and Apply changes nothing.
func TestTuneLockedByEnv(t *testing.T) {
	home := tuneSandbox(t)
	path := filepath.Join(home, ".claude", "settings.json")
	tuneWrite(t, path, `{"env": {"CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL": "1h"}}`)
	s, _ := TuneRead(home, "claude", TuneSubCache, "")
	if s.Locked != "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL" || s.Value != "1h" {
		t.Fatalf("read %+v", s)
	}
	if err := TuneApply(home, "claude", TuneSubCache, "", "5m"); err == nil {
		t.Fatal("applied under a variable that takes precedence")
	}
	if got := tuneFile(t, path); strings.Contains(got, "subagentPromptCacheTtl") {
		t.Fatalf("wrote anyway:\n%s", got)
	}
	t.Setenv("FORCE_PROMPT_CACHING_5M", "1")
	if s, _ := TuneRead(home, "claude", TuneCache, ""); s.Locked != "FORCE_PROMPT_CACHING_5M" || s.Value != "5m" {
		t.Fatalf("read %+v", s)
	}
}

// TestTuneCodexCompact: Codex's limit goes at the top of config.toml, the
// tables after it kept, and Undo puts the user's back.
func TestTuneCodexCompact(t *testing.T) {
	home := tuneSandbox(t)
	path := filepath.Join(home, ".codex", "config.toml")
	tuneWrite(t, path, "model = \"gpt-5.5\"\nmodel_auto_compact_token_limit = 200000\n\n[features]\nx = true\n")
	if err := TuneApply(home, "codex", TuneCompact, "", "150000"); err != nil {
		t.Fatal(err)
	}
	got := tuneFile(t, path)
	if !strings.Contains(got, "model_auto_compact_token_limit = 150000") || strings.Index(got, "150000") > strings.Index(got, "[features]") || !strings.Contains(got, "x = true") {
		t.Fatalf("config.toml after apply:\n%s", got)
	}
	if err := TuneUndo(home, "codex", TuneCompact, ""); err != nil {
		t.Fatal(err)
	}
	if got := tuneFile(t, path); !strings.Contains(got, "model_auto_compact_token_limit = 200000") {
		t.Fatalf("config.toml after undo:\n%s", got)
	}
}
