package provider

import (
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
)

func TestParseCursorModels(t *testing.T) {
	out := "\x1b[2K\x1b[GAvailable models\n\nauto - Auto (default)\ngpt-5.3-codex-high - Codex 5.3 High\nclaude-opus-5-thinking-high - Claude Opus 5 Thinking High (current)\n\nTip: use --model <id>\n"
	ms := parseCursorModels(out)
	want := [][2]string{{"auto", "Auto"}, {"gpt-5.3-codex-high", "Codex 5.3 High"}, {"claude-opus-5-thinking-high", "Claude Opus 5 Thinking High"}}
	if len(ms) != len(want) {
		t.Fatalf("got %d models: %+v", len(ms), ms)
	}
	for i, w := range want {
		if ms[i].ID != w[0] || ms[i].Name != w[1] {
			t.Errorf("model %d = %q %q, want %q %q", i, ms[i].ID, ms[i].Name, w[0], w[1])
		}
	}
}

// cursorListed is some of `cursor-agent models`, as it prints it.
const cursorListed = "Available models\n\nauto - Auto (default)\n" +
	"gpt-5.3-codex-low - Codex 5.3 Low\ngpt-5.3-codex - Codex 5.3\ngpt-5.3-codex-high - Codex 5.3 High\ngpt-5.3-codex-high-fast - Codex 5.3 High Fast\n" +
	"grok-4.7-low - Grok 4.7  Low\ngrok-4.7-low-fast - Grok 4.7  Low Fast​​\ngrok-4.7-medium - Grok 4.7  Medium\ngrok-4.7-medium-fast - Grok 4.7  Medium Fast​​\n" +
	"grok-4.7-xhigh - Grok 4.7  Extra High\ngrok-4.7-xhigh-fast - Grok 4.7  Extra High Fast​​\n" +
	"claude-opus-4-7-low - Claude Opus 4.7 1M Low\nclaude-opus-4-7-xhigh - Claude Opus 4.7 1M\nclaude-opus-4-7-max - Claude Opus 4.7 1M Max\n" +
	"claude-opus-4-7-thinking-low - Claude Opus 4.7 1M Low Thinking\nclaude-opus-4-7-thinking-xhigh - Claude Opus 4.7 1M Thinking\n" +
	"claude-4.6-opus-high-thinking - Claude Opus 4.6 1M Thinking\nclaude-4.6-opus-max-thinking - Claude Opus 4.6 1M Max Thinking\n" +
	"gpt-5.5-none - GPT-5.5 1M None\ngpt-5.5-medium - GPT-5.5 1M\ngpt-5.5-extra-high - GPT-5.5 1M Extra High\n" +
	"gpt-5.5-medium-fast - GPT-5.5 Fast\ngpt-5.5-extra-high-fast - GPT-5.5 Extra High Fast\n" +
	"claude-4.6-sonnet-medium-thinking - Claude Sonnet 4.6 1M Thinking\ncomposer-2.5 - Composer 2.5\n\nTip: use --model <id>\n"

// Each family of Cursor's ids is one model, with the efforts it has; fast
// and thinking are families of their own, and a family of one keeps its id.
func TestCollapseCursorModels(t *testing.T) {
	raw := withCursorContexts(parseCursorModels(cursorListed))
	if raw[5].Name != "Grok 4.7 Low" || raw[6].Name != "Grok 4.7 Low Fast" {
		t.Fatalf("%q %q", raw[5].Name, raw[6].Name)
	}
	var got []string
	for _, m := range collapseCursorModels(raw) {
		got = append(got, fmt.Sprintf("%s|%s|%d|%s", m.ID, m.Name, m.Context, strings.Join(m.Efforts, ",")))
	}
	want := []string{
		"auto|Auto|200000|",
		"gpt-5.3-codex|Codex 5.3|200000|low,high",
		"gpt-5.3-codex-high-fast|Codex 5.3 High Fast|200000|",
		"grok-4.7|Grok 4.7|200000|low,medium,xhigh",
		"grok-4.7-fast|Grok 4.7 Fast|200000|low,medium,xhigh",
		"claude-opus-4-7|Claude Opus 4.7 1M|1000000|low,xhigh,max",
		"claude-opus-4-7-thinking|Claude Opus 4.7 1M Thinking|1000000|low,xhigh",
		"claude-4.6-opus-thinking|Claude Opus 4.6 1M Thinking|1000000|high,max",
		"gpt-5.5|GPT-5.5 1M|1000000|none,medium,xhigh",
		"gpt-5.5-fast|GPT-5.5 Fast|200000|medium,xhigh", // Cursor doesn't say 1M of it
		"claude-4.6-sonnet-medium-thinking|Claude Sonnet 4.6 1M Thinking|1000000|",
		"composer-2.5|Composer 2.5|200000|",
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("got\n%s\nwant\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	for _, c := range []struct{ model, effort, want string }{
		{"gpt-5.3-codex", "", "gpt-5.3-codex"},
		{"grok-4.7", "", "grok-4.7-medium"},
		{"claude-opus-4-7", "", "claude-opus-4-7-xhigh"}, // its name doesn't say its effort
		{"claude-opus-4-7-thinking", "low", "claude-opus-4-7-thinking-low"},
		{"claude-4.6-opus-thinking", "max", "claude-4.6-opus-max-thinking"},
		{"gpt-5.5", "xhigh", "gpt-5.5-extra-high"},
		{"gpt-5.5-fast", "", "gpt-5.5-medium-fast"},
	} {
		vs, ok := cursorVariantsIn(raw, c.model)
		if !ok || vs[c.effort] != c.want {
			t.Errorf("%s at %q: %v", c.model, c.effort, vs)
		}
	}
	for _, id := range []string{"composer-2.5", "gpt-5.3-codex-high-fast", "grok-4.7-low", "nope"} {
		if _, ok := cursorVariantsIn(raw, id); ok {
			t.Errorf("%s stands for a family", id)
		}
	}
}

// Picks saved before Cursor's efforts were one model name Cursor's own
// ids: each is the model magpie offers for it now, once, where it was,
// with its efforts; ids of a family of one, or none, stay.
func TestCursorLegacyPicks(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	was := cursorStatus
	defer func() { cursorStatus = was }()
	cursorStatus = &cliIdentity{name: "cursor-test", exe: func() string { return "/bin/sh" }, ask: func() (string, string, bool, error) { return "me@example.com", "Pro", true, nil }}
	raw := withCursorContexts(parseCursorModels(cursorListed))
	if err := catalog.SaveLive("cursor", "", raw); err != nil {
		t.Fatal(err)
	}
	for id, want := range map[string]string{"grok-4.7-low": "grok-4.7|low", "grok-4.7-xhigh-fast": "grok-4.7-fast|xhigh",
		"claude-4.6-opus-high-thinking": "claude-4.6-opus-thinking|high", "gpt-5.5-extra-high": "gpt-5.5|xhigh"} {
		if b, e, ok := cursorBaseIn(raw, id); !ok || b+"|"+e != want {
			t.Errorf("%s: %s|%s %v, want %s", id, b, e, ok, want)
		}
	}
	for _, id := range []string{"grok-4.7", "gpt-5.3-codex-high-fast", "composer-2.5", "claude-4.6-sonnet-medium-thinking", "grok-4.7-high", "nope-low"} {
		if b, _, ok := cursorBaseIn(raw, id); ok {
			t.Errorf("%s taken for %s", id, b)
		}
	}
	legacy := []string{"grok-4.7-low", "composer-2.5", "grok-4.7-medium", "grok-4.7-low-fast", "grok-4.7-xhigh", "grok-4.7-xhigh-fast", "gpt-5.3-codex-high-fast", "gone-model"}
	if err := Save(Provider{ID: "cursor", Models: legacy}); err != nil {
		t.Fatal(err)
	}
	p, err := Find("cursor")
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"grok-4.7", "composer-2.5", "grok-4.7-fast", "gpt-5.3-codex-high-fast", "gone-model"}
	if strings.Join(p.Models, ",") != strings.Join(want, ",") {
		t.Fatalf("picks %v, want %v", p.Models, want)
	}
	var got []string
	for _, m := range p.Exposed() {
		got = append(got, m.ID+"|"+strings.Join(m.Efforts, ","))
	}
	if strings.Join(got, " ") != "grok-4.7|low,medium,xhigh composer-2.5| grok-4.7-fast|low,medium,xhigh gpt-5.3-codex-high-fast| gone-model|" {
		t.Fatalf("exposed %v", got)
	}
}

// Cursor's names say which of its models hold 1M; the rest hold its 200K
func TestCursorContext(t *testing.T) {
	for _, c := range []struct {
		id, name string
		want     int
	}{
		{"claude-opus-5-5-high-fast", "Claude Opus 5.5 1M High Fast", 1_000_000},
		{"gpt-5.6-sol-xhigh", "GPT-5.6 Sol 1M Extra High", 1_000_000},
		{"gpt-5.5-medium-fast", "GPT-5.5 Fast", 200_000},
		{"composer-2.5", "Composer 2.5", 200_000},
		{"auto", "Auto", 200_000},
	} {
		if got := cursorContext(c.id, c.name); got != c.want {
			t.Errorf("%s (%s): %d, want %d", c.id, c.name, got, c.want)
		}
	}
	if ms := withCursorContexts(parseCursorModels("x-1 - X 1M\n")); ms[0].Context != 1_000_000 {
		t.Fatalf("%+v", ms)
	}
}

// The API is told the installed CLI's version: where its executable lives,
// else the newest one installed, else one known to work.
func TestCursorClientVersion(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	exe := CursorExecutable
	defer func() { CursorExecutable = exe }()
	CursorExecutable = func() string { return "" }
	if v := CursorClientVersion(); v != "cli-"+cursorVersionFallback {
		t.Fatal(v)
	}
	versions := filepath.Join(home, ".local", "share", "cursor-agent", "versions")
	for _, v := range []string{"2026.08.01-abc123", "2026.09.01-def456", "notes"} {
		os.MkdirAll(filepath.Join(versions, v), 0o755)
	}
	if v := CursorClientVersion(); v != "cli-2026.09.01-def456" {
		t.Fatal(v)
	}
	bin := filepath.Join(versions, "2026.08.01-abc123", "cursor-agent")
	os.WriteFile(bin, nil, 0o755)
	link := filepath.Join(home, "cursor-agent")
	if os.Symlink(bin, link) == nil {
		CursorExecutable = func() string { return link }
		if v := CursorClientVersion(); v != "cli-2026.08.01-abc123" {
			t.Fatal(v)
		}
	}
}

func TestTokenExpiry(t *testing.T) {
	jwt := func(claims string) string {
		return "e30." + base64.RawURLEncoding.EncodeToString([]byte(claims)) + ".sig"
	}
	if got := tokenExpiry(jwt(`{"exp":1790000000}`)); !got.Equal(time.Unix(1790000000, 0)) {
		t.Fatal(got)
	}
	for _, tok := range []string{"", "opaque", jwt(`{"sub":"x"}`), "a.!!.c"} {
		if !tokenExpiry(tok).IsZero() {
			t.Fatal(tok)
		}
	}
}

// Cursor's plugin gives each model Cursor's 200K unless the name says 1M;
// a model known to hold less keeps its own there, as the built-in's list.
func TestCursorPluginContext(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755)
	os.WriteFile(catalog.CachePath(), []byte(`{
	  "openai":{"id":"openai","models":{"gpt-4o":{"id":"gpt-4o","limit":{"context":128000,"output":16384}}}},
	  "moonshotai":{"id":"moonshotai","models":{"kimi-k2":{"id":"kimi-k2","limit":{"context":131072,"output":16384}}}}}`), 0o644)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	pp := plugin.Provider{ID: "cursor", Models: []plugin.Model{
		{ID: "gpt-4o", Name: "GPT-4o", Context: 200_000},
		{ID: "kimi-k2-high", Name: "Kimi K2", Context: 200_000},
		{ID: "claude-opus-5-5-high", Name: "Claude Opus 5.5", Context: 1_000_000},
		{ID: "composer-2.5", Name: "Composer 2.5", Context: 200_000},
	}}
	got := map[string]int{}
	for _, m := range pluginCatalog(pp) {
		got[m.ID] = m.Context
	}
	want := map[string]int{"gpt-4o": 128_000, "kimi-k2-high": 131_072, "claude-opus-5-5-high": 1_000_000, "composer-2.5": 200_000}
	for id, n := range want {
		if got[id] != n {
			t.Errorf("%s: %d, want %d", id, got[id], n)
		}
	}
	// another plugin's models are its own
	pp.ID = "fakeco"
	if c := pluginCatalog(pp)[0].Context; c != 200_000 {
		t.Fatalf("fakeco gpt-4o: %d", c)
	}
}
