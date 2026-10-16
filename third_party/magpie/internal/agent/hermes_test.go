package agent

import (
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"gopkg.in/yaml.v3"
)

func TestHermes(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("HERMES_HOME", "")
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro", "flash"}}); err != nil {
		t.Fatal(err)
	}
	dir := appdir.HermesHomeIn(home)
	path := filepath.Join(dir, "config.yaml")
	read := func() (map[string]any, map[string]any, string) {
		var c map[string]any
		b, _ := os.ReadFile(path)
		yaml.Unmarshal(b, &c)
		m, _ := c["model"].(map[string]any)
		return c, m, string(b)
	}
	providers := func(c map[string]any) map[string]any { p, _ := c["providers"].(map[string]any); return p }
	os.MkdirAll(dir, 0o755)
	os.WriteFile(path, []byte("# mine\nmodel:\n  default: anthropic/claude-opus-5 # main\n  provider: openrouter\n  base_url: https://openrouter.ai/api/v1\nproviders:\n  mine:\n    base_url: https://x\n"), 0o644)

	f := hermes(home).Field("model")
	if f.Get() != "anthropic/claude-opus-5" {
		t.Fatalf("get: %q", f.Get())
	}
	if err := f.Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	c, m, raw := read()
	mp, _ := providers(c)["magpie"].(map[string]any)
	if m["default"] != "deepseek/pro" || m["provider"] != "magpie" || m["base_url"] != "https://openrouter.ai/api/v1" ||
		providers(c)["mine"] == nil || mp == nil || mp["api_mode"] != "chat_completions" || !strings.HasSuffix(mp["base_url"].(string), "/v1") ||
		!strings.Contains(raw, "# mine") {
		t.Fatalf("config:\n%s", raw)
	}
	ids := map[string]bool{}
	for _, x := range mp["models"].([]any) {
		ids[x.(string)] = true
	}
	if !ids["deepseek/pro"] || !ids["deepseek/flash"] {
		t.Fatalf("models: %v", ids)
	}
	if f.Get() != "magpie/deepseek/pro" {
		t.Fatalf("get: %q", f.Get())
	}
	// another magpie model keeps what was stashed first
	if err := f.Set("magpie/deepseek/flash"); err != nil {
		t.Fatal(err)
	}

	// its own model: the user's provider comes back, magpie's entry stays
	// beside it, so Hermes's session picker still offers magpie's models
	// and it is still connected (lijiho96940561 on X)
	if err := f.Set("moonshotai/kimi-k3"); err != nil {
		t.Fatal(err)
	}
	c, m, raw = read()
	if m["default"] != "moonshotai/kimi-k3" || m["provider"] != "openrouter" || providers(c)["magpie"] == nil || providers(c)["mine"] == nil {
		t.Fatalf("own:\n%s", raw)
	}
	a := hermes(home)
	if !a.Wired() || a.Drift() != nil {
		t.Fatalf("own model: wired %v, drift %+v", a.Wired(), a.Drift())
	}

	// the default changed in Hermes itself (`hermes model`): still connected,
	// not told as drift
	if err := a.Apply("model", "magpie/deepseek/flash"); err != nil {
		t.Fatal(err)
	}
	if err := edit.SetYAML(path, edit.KV{Path: "model.provider", Value: "anthropic"}, edit.KV{Path: "model.default", Value: "claude-opus-5"}); err != nil {
		t.Fatal(err)
	}
	if !a.Wired() || a.Drift() != nil {
		t.Fatalf("changed in Hermes: wired %v, drift %+v", a.Wired(), a.Drift())
	}

	// Disconnect takes magpie's entry out
	if err := a.Disconnect(); err != nil {
		t.Fatal(err)
	}
	c, m, raw = read()
	if m["default"] != "claude-opus-5" || m["provider"] != "anthropic" || providers(c)["magpie"] != nil || providers(c)["mine"] == nil || a.Wired() {
		t.Fatalf("disconnect, own model:\n%s", raw)
	}

	// Disconnect from magpie's model: back to what the user had
	f.Set("magpie/deepseek/flash")
	if err := a.Disconnect(); err != nil {
		t.Fatal(err)
	}
	c, m, raw = read()
	if m["default"] != "claude-opus-5" || m["provider"] != "anthropic" || providers(c)["magpie"] != nil {
		t.Fatalf("disconnect:\n%s", raw)
	}

	// a config with no provider of its own: none comes back
	os.WriteFile(path, []byte("agent:\n  max_turns: 60\n"), 0o644)
	f.Set("magpie/deepseek/pro")
	if err := a.Disconnect(); err != nil {
		t.Fatal(err)
	}
	c, m, raw = read()
	if m["provider"] != nil || m["default"] != nil || providers(c)["magpie"] != nil || c["agent"] == nil {
		t.Fatalf("disconnect, no provider:\n%s", raw)
	}
}

// Each of Hermes's bots is a profile under ~/.hermes/profiles/<name> with a
// config.yaml of its own, which doesn't read the main one's: connected,
// magpie is offered in every bot's picker, each keeps its own model, and
// Disconnect takes magpie out of each (lijiho96940561 on X: only the main
// bot had magpie).
func TestHermesProfiles(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("HERMES_HOME", "")
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	root := appdir.HermesHomeIn(home)
	write := func(rel, s string) string {
		p := filepath.Join(root, rel)
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte(s), 0o644)
		return p
	}
	write("config.yaml", "model:\n  default: anthropic/claude-opus-5\n  provider: openrouter\n")
	// as `hermes profile create` leaves them: config.yaml, .env, SOUL.md
	bot := write("profiles/coder/config.yaml", "# coder bot\nmodel:\n  default: kimi-k3\n  provider: moonshot\nproviders:\n  mine:\n    base_url: https://x\n")
	write("profiles/coder/SOUL.md", "coder\n")
	// cloned from a main config already on magpie
	clone := write("profiles/writer/config.yaml", "model:\n  default: deepseek/pro\n  provider: magpie\n")
	// deleted (a tombstone beside it), and a name Hermes doesn't allow
	gone := write("profiles/old/config.yaml", "model:\n  default: x\n")
	write("profiles/.deleted/old", "")
	bad := write("profiles/Bad/config.yaml", "model:\n  default: x\n")
	read := func(p string) (map[string]any, string) {
		var c map[string]any
		b, _ := os.ReadFile(p)
		yaml.Unmarshal(b, &c)
		return c, string(b)
	}
	has := func(p string) bool {
		c, _ := read(p)
		ps, _ := c["providers"].(map[string]any)
		return ps["magpie"] != nil
	}

	a := hermes(home)
	if err := a.Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	c, raw := read(bot)
	m, _ := c["model"].(map[string]any)
	if !has(bot) || !has(clone) || m["default"] != "kimi-k3" || m["provider"] != "moonshot" || !strings.Contains(raw, "# coder bot") ||
		c["providers"].(map[string]any)["mine"] == nil {
		t.Fatalf("coder after connect:\n%s", raw)
	}
	if has(gone) || has(bad) {
		t.Fatal("a deleted or invalid profile got magpie")
	}

	// a profile made later gets it at the next sync
	later := write("profiles/later/config.yaml", "model:\n  default: x\n")
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if !has(later) {
		t.Fatal("sync left a new profile out")
	}

	if err := a.Disconnect(); err != nil {
		t.Fatal(err)
	}
	c, raw = read(bot)
	m, _ = c["model"].(map[string]any)
	if has(bot) || has(later) || m["default"] != "kimi-k3" || m["provider"] != "moonshot" || c["providers"].(map[string]any)["mine"] == nil {
		t.Fatalf("coder after disconnect:\n%s", raw)
	}
	// the clone isn't left naming a provider that is gone
	c, raw = read(clone)
	m, _ = c["model"].(map[string]any)
	if has(clone) || m["provider"] != nil || m["default"] != nil {
		t.Fatalf("writer after disconnect:\n%s", raw)
	}
	// disconnected: a sync doesn't put it back
	if err := a.Sync(); err != nil || has(bot) {
		t.Fatalf("sync after disconnect: %v", err)
	}
}

// HERMES_HOME set to a profile outside ~/.hermes (Docker): magpie connects
// that profile, and the root's config and its other profiles get the
// provider too
func TestHermesProfilesOfCustomRoot(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	root := filepath.Join(home, "data")
	for _, p := range []string{"config.yaml", "profiles/a/config.yaml", "profiles/b/config.yaml"} {
		os.MkdirAll(filepath.Dir(filepath.Join(root, p)), 0o755)
		os.WriteFile(filepath.Join(root, p), []byte("model:\n  default: x\n"), 0o644)
	}
	t.Setenv("HERMES_HOME", filepath.Join(root, "profiles", "a"))
	if got := hermesProfiles(here(home)); len(got) != 3 {
		t.Fatalf("profiles: %v", got)
	}
	if err := hermes(home).Apply("model", "magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{"config.yaml", "profiles/b/config.yaml"} {
		v, ok := edit.GetYAML(filepath.Join(root, p), "providers.magpie.base_url")
		if !ok || !strings.HasSuffix(v, "/v1") {
			t.Fatalf("%s: %q", p, v)
		}
	}
	if v, _ := edit.GetYAML(filepath.Join(root, "config.yaml"), "model.default"); v != "x" {
		t.Fatalf("root model: %q", v)
	}
}

// TestHermesFolder (新v学员 on Discord): on this machine Hermes's folder is
// its own default, %LOCALAPPDATA%\hermes on native Windows, not ~/.hermes
// there, and its profiles are under it; in a WSL distro, which is Linux and
// whose variables magpie can't read, it is ~/.hermes.
func TestHermesFolder(t *testing.T) {
	home := t.TempDir()
	local := filepath.Join(home, "AppData", "Local")
	t.Setenv("LOCALAPPDATA", local)
	t.Setenv("HERMES_HOME", "")
	want := filepath.Join(home, ".hermes")
	if runtime.GOOS == "windows" {
		want = filepath.Join(local, "hermes")
	}
	if a := hermes(home); a.Dir != want || a.Path != filepath.Join(want, "config.yaml") {
		t.Fatalf("Hermes at %s (%s), want %s", a.Dir, a.Path, want)
	}
	os.MkdirAll(filepath.Join(want, "profiles", "bot"), 0o755)
	os.WriteFile(filepath.Join(want, "profiles", "bot", "config.yaml"), []byte("model: {}\n"), 0o644)
	if got := hermesProfiles(here(home)); len(got) != 2 || got[0] != filepath.Join(want, "config.yaml") {
		t.Fatalf("profiles %v, want under %s", got, want)
	}
	t.Setenv("HERMES_HOME", filepath.Join(home, "custom"))
	wsl := place{home: home, spell: func(s string) string { return s }}
	if a := hermesIn(wsl); a.Dir != filepath.Join(home, ".hermes") {
		t.Fatalf("Hermes in WSL at %s, want ~/.hermes", a.Dir)
	}
}

// What the user, or Hermes itself, set on magpie's provider stays through
// every sync and pick, in the connected config and in each profile's
// (#2030's siblings): the default_model `hermes model` writes there, a name,
// a timeout, a key command, a header beside magpie's. The address, key, API
// mode and model list stay magpie's.
func TestHermesKeepsTheUsersSettings(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("HERMES_HOME", "")
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	root := appdir.HermesHomeIn(home)
	path := filepath.Join(root, "config.yaml")
	bot := filepath.Join(root, "profiles", "coder", "config.yaml")
	os.MkdirAll(filepath.Dir(bot), 0o755)
	os.WriteFile(path, []byte("model:\n  default: anthropic/claude-opus-5\n  provider: openrouter\n"), 0o644)
	os.WriteFile(bot, []byte("model:\n  default: kimi-k3\n  provider: moonshot\n"), 0o644)
	a := hermes(home)
	f := a.Field("model")
	if err := f.Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{path, bot} {
		if err := edit.SetYAML(p,
			edit.KV{Path: "providers.magpie.name", Value: "My GW"},
			edit.KV{Path: "providers.magpie.default_model", Value: "deepseek/pro"},
			edit.KV{Path: "providers.magpie.request_timeout_seconds", Value: 900},
			edit.KV{Path: "providers.magpie.key_cmd", Value: "cat ~/.gw-key"},
			edit.KV{Path: "providers.magpie.extra_headers.X-Mine", Value: "1"},
			edit.KV{Path: "providers.magpie.base_url", Value: "http://127.0.0.1:1/v1"},
			edit.KV{Path: "providers.magpie.models", Value: []string{"gone/old"}},
		); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.Save(provider.Provider{ID: "kimi", Name: "Kimi", Chat: "https://api.moonshot.ai/v1", Key: "k", Models: []string{"k3"}}); err != nil {
		t.Fatal(err)
	}
	for range 3 {
		if err := a.Sync(); err != nil {
			t.Fatal(err)
		}
	}
	check := func(when string) {
		t.Helper()
		for _, p := range []string{path, bot} {
			var c struct {
				Providers map[string]struct {
					Name    string            `yaml:"name"`
					Default string            `yaml:"default_model"`
					Timeout int               `yaml:"request_timeout_seconds"`
					KeyCmd  string            `yaml:"key_cmd"`
					Headers map[string]string `yaml:"extra_headers"`
					BaseURL string            `yaml:"base_url"`
					APIKey  string            `yaml:"api_key"`
					APIMode string            `yaml:"api_mode"`
					Models  []string          `yaml:"models"`
				} `yaml:"providers"`
			}
			raw := readFile(p)
			if err := yaml.Unmarshal([]byte(raw), &c); err != nil {
				t.Fatalf("%v\n%s", err, raw)
			}
			mp := c.Providers[magpieID]
			if mp.Name != "My GW" || mp.Default != "deepseek/pro" || mp.Timeout != 900 || mp.KeyCmd != "cat ~/.gw-key" || mp.Headers["X-Mine"] != "1" {
				t.Errorf("%s: the user's settings not kept in %s:\n%s", when, p, raw)
			}
			if mp.BaseURL == "http://127.0.0.1:1/v1" || mp.APIKey == "" || mp.APIMode != "chat_completions" || mp.Headers["User-Agent"] != "hermes-agent" ||
				slices.Contains(mp.Models, "gone/old") || !slices.Contains(mp.Models, "kimi/k3") {
				t.Errorf("%s: magpie's own parts not written in %s:\n%s", when, p, raw)
			}
		}
	}
	check("sync")
	if err := f.Set("magpie/kimi/k3"); err != nil {
		t.Fatal(err)
	}
	check("pick")
}

// PaulT on Discord: a key made under Gateway keys, put in Hermes's
// config.yaml in place of magpie's, goes through magpie all the same, so
// the check is quiet and a sync keeps it. A key the gateway doesn't take
// still is drift.
func TestHermesWithAGatewayKeyOfTheUsers(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("HERMES_HOME", "")
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Chat: "https://api.deepseek.com/v1", Key: "k", Models: []string{"pro"}}); err != nil {
		t.Fatal(err)
	}
	dir := appdir.HermesHomeIn(home)
	path := filepath.Join(dir, "config.yaml")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(path, []byte("model:\n  default: x\n"), 0o644)
	a := hermes(home)
	if err := a.Field("model").Set("magpie/deepseek/pro"); err != nil {
		t.Fatal(err)
	}
	if d := a.Check(); d != "" {
		t.Fatalf("as magpie wrote it: %s", d)
	}
	secret, err := access.Update("add-key", access.Change{Name: "hermes"})
	if err != nil {
		t.Fatal(err)
	}
	if err := edit.SetYAML(path, edit.KV{Path: "providers.magpie.api_key", Value: secret}); err != nil {
		t.Fatal(err)
	}
	if d := a.Check(); d != "" {
		t.Fatalf("a key the gateway issued is drift: %s", d)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if v, _ := edit.GetYAML(path, "providers.magpie.api_key"); v != secret {
		t.Fatalf("a sync wrote over the user's key: %q", v)
	}
	if err := edit.SetYAML(path, edit.KV{Path: "providers.magpie.api_key", Value: "sk-not-this-gateways"}); err != nil {
		t.Fatal(err)
	}
	if d := a.Check(); !strings.Contains(d, "api_key") {
		t.Fatalf("a key the gateway doesn't take is not reported: %q", d)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if v, _ := edit.GetYAML(path, "providers.magpie.api_key"); v != gateway.Token {
		t.Fatalf("a key the gateway doesn't take is kept: %q", v)
	}
}

// keyHolds, which every agent's check of magpie's key goes through: a key
// of the gateway's own stands in for magpie's, never for a URL or another
// value, and one the gateway doesn't take doesn't.
func TestKeyHoldsTakesAGatewayKey(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	secret, err := access.Update("add-key", access.Change{Name: "mine"})
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct {
		v, want string
		ok      bool
	}{
		{gateway.Token, gateway.Token, true},
		{secret, gateway.Token, true},
		{secret, gateway.TokenFor("alma"), true},
		{"sk-other", gateway.Token, false},
		{"", gateway.Token, false},
		{secret, "http://127.0.0.1:3425/v1", false},
		{secret, "chat_completions", false},
	} {
		if got := keyHolds(c.v, c.want); got != c.ok {
			t.Errorf("keyHolds(%q, %q) = %v", c.v, c.want, got)
		}
	}
	if d := wiringOff("X", "x.yaml", func(string) (string, bool) { return secret, true }, "api_key", gateway.Token); d != "" {
		t.Fatalf("wiringOff: %s", d)
	}
}
