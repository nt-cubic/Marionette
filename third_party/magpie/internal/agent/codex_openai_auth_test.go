package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Qyburn3 on X: with magpie as Codex's provider for a Codex signed in to
// ChatGPT, the Codex app (ChatGPT.app 26.1021) lost its ChatGPT account:
// app-server's account/read and getAuthStatus answer no account and no
// token for a provider without requires_openai_auth, and the app's
// ChatGPT mode showed no chats or projects ("Work" sessions gone) while
// magpie's models still answered. The table says requires_openai_auth
// while that sign-in has room, and not for one the Codex app holds (Send
// would grey again, #540), an API key (its image tool), or no sign-in (a
// login screen). Sync brings a table an older magpie wrote up to date,
// both ways.
func TestCodexMagpieProviderKeepsChatGPTAccount(t *testing.T) {
	// the table as an older magpie wrote it, bytes from a live config.toml
	const older = "model = \"fake/m1\"\nmodel_provider = \"magpie\"\nmodel_catalog_json = \"magpie-models.json\"\n\n" +
		"[model_providers.magpie]\nname = \"magpie\"\nbase_url = \"http://127.0.0.1:3425/v1\"\nwire_api = \"responses\"\n" +
		"experimental_bearer_token = \"magpie\"\nhttp_headers = { \"x-openai-actor-authorization\" = \"magpie\" }\n"
	flag := func(t *testing.T, cfg string) string {
		t.Helper()
		i := strings.Index(cfg, "[model_providers.magpie]")
		if i < 0 {
			t.Fatalf("no magpie provider table:\n%s", cfg)
		}
		rest := cfg[i+1:]
		if j := strings.Index(rest, "\n["); j >= 0 {
			rest = rest[:j]
		}
		for _, l := range strings.Split(rest, "\n") {
			if k, v, ok := strings.Cut(l, "="); ok && strings.TrimSpace(k) == "requires_openai_auth" {
				return strings.TrimSpace(v)
			}
		}
		return ""
	}

	t.Run("ChatGPT, magpie API", func(t *testing.T) {
		home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, "")
		cx := codex(home)
		if err := cx.Field("login").Set("api"); err != nil {
			t.Fatal(err)
		}
		if err := cx.Fields[0].Set("fake/m1"); err != nil {
			t.Fatal(err)
		}
		cfg := read()
		if !strings.Contains(cfg, `model_provider = "magpie"`) || flag(t, cfg) != "true" {
			t.Fatalf("magpie its provider beside a ChatGPT sign-in with room: requires_openai_auth wanted:\n%s", cfg)
		}
		if !strings.Contains(cfg, `"x-openai-actor-authorization" = "magpie"`) || !strings.Contains(cfg, "experimental_bearer_token") {
			t.Fatalf("magpie's key and actor header kept:\n%s", cfg)
		}
		// the account runs out: the takeover's way, no account in the app
		codexUsedUp = func() bool { return true }
		if err := cx.Sync(); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); flag(t, cfg) != "" {
			t.Fatalf("used up: no requires_openai_auth, or Send greys (#540):\n%s", cfg)
		}
		codexUsedUp = func() bool { return false }
		if err := cx.Sync(); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); flag(t, cfg) != "true" {
			t.Fatalf("room again: requires_openai_auth back:\n%s", cfg)
		}
		if c := cx.Check(); c != "" {
			t.Fatal(c)
		}
	})

	t.Run("ChatGPT, taken over", func(t *testing.T) {
		home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, "")
		cx := codex(home)
		codexUsedUp = func() bool { return true }
		if err := cx.Fields[0].Set("fake/m1"); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); !strings.Contains(cfg, `model_provider = "magpie"`) || flag(t, cfg) != "" {
			t.Fatalf("codex.out: magpie its provider with no requires_openai_auth:\n%s", cfg)
		}
		// the reset: back beside the sign-in, the table kept for the
		// threads started on magpie, now keeping the account too
		codexUsedUp = func() bool { return false }
		if err := cx.Sync(); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); strings.Contains(cfg, "model_provider =") || flag(t, cfg) != "true" {
			t.Fatalf("after the reset:\n%s", cfg)
		}
	})

	// 蛋仔 on Discord: requires_openai_auth = false written by hand, to have
	// the Codex app on magpie with no ChatGPT account, was set back to true
	// by the next sync while the sign-in had room
	t.Run("user's own false kept", func(t *testing.T) {
		home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, "")
		cx := codex(home)
		if err := cx.Field("login").Set("api"); err != nil {
			t.Fatal(err)
		}
		if err := cx.Fields[0].Set("fake/m1"); err != nil {
			t.Fatal(err)
		}
		cfg := read()
		cfg = strings.Replace(cfg, "requires_openai_auth = true", "requires_openai_auth = false", 1)
		if err := os.WriteFile(filepath.Join(home, ".codex", "config.toml"), []byte(cfg), 0o600); err != nil {
			t.Fatal(err)
		}
		for _, used := range []bool{false, true, false} {
			codexUsedUp = func() bool { return used }
			if err := cx.Sync(); err != nil {
				t.Fatal(err)
			}
			if cfg := read(); flag(t, cfg) != "false" {
				t.Fatalf("used up %v: the user's requires_openai_auth = false kept:\n%s", used, cfg)
			}
		}
		codexUsedUp = func() bool { return false }
		if err := cx.Fields[0].Set("fake/m2"); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); flag(t, cfg) != "false" {
			t.Fatalf("a model switch: the user's false kept:\n%s", cfg)
		}
		if c := cx.Check(); c != "" {
			t.Fatal(c)
		}
	})

	t.Run("older table synced", func(t *testing.T) {
		home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, older)
		cx := codex(home)
		if err := cx.Field("login").Set("api"); err != nil {
			t.Fatal(err)
		}
		if err := cx.Sync(); err != nil {
			t.Fatal(err)
		}
		if cfg := read(); flag(t, cfg) != "true" {
			t.Fatalf("an older magpie's table, synced: requires_openai_auth wanted:\n%s", cfg)
		}
	})

	for _, c := range []struct{ name, auth string }{
		{"API key", `{"OPENAI_API_KEY":"sk-relay"}`},
		{"no sign-in", ""},
	} {
		t.Run(c.name, func(t *testing.T) {
			home, read := codexHome(t, c.auth, "")
			cx := codex(home)
			if err := cx.Fields[0].Set("fake/m1"); err != nil {
				t.Fatal(err)
			}
			if cfg := read(); !strings.Contains(cfg, `model_provider = "magpie"`) || flag(t, cfg) != "" {
				t.Fatalf("no ChatGPT account: no requires_openai_auth:\n%s", cfg)
			}
		})
		// one written while signed in, the sign-in gone since
		t.Run(c.name+" after ChatGPT", func(t *testing.T) {
			withFlag := strings.Replace(older, "wire_api = \"responses\"\n", "wire_api = \"responses\"\nrequires_openai_auth = true\n", 1)
			home, read := codexHome(t, c.auth, withFlag)
			cx := codex(home)
			if err := cx.Sync(); err != nil {
				t.Fatal(err)
			}
			if cfg := read(); flag(t, cfg) != "" {
				t.Fatalf("signed out: requires_openai_auth goes:\n%s", cfg)
			}
		})
	}
}
