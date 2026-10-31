package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Tiger on Discord ("重启 codex 之后登不回去了"): a config.toml with
// forced_login_method = "api", written by another tool or guide, makes
// Codex drop a ChatGPT sign-in (the TUI logs it out on start, the app
// server reads no account). Wired beside that sign-in, through
// openai_base_url and the built-in provider, the Codex app asked for a
// sign-in and offered an API key alone. Such a sign-in is none to magpie:
// magpie is the provider, without requires_openai_auth, so Codex needs no
// sign-in, and the user's line stays. Taking the line out brings the
// ChatGPT account back to the Codex app (requires_openai_auth).
func TestCodexForcedAPILoginIsNoChatGPT(t *testing.T) {
	home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, "forced_login_method = \"api\"\n")
	cx := codex(home)
	if err := cx.Fields[0].Set("fake/m1"); err != nil {
		t.Fatal(err)
	}
	cfg := read()
	if !strings.Contains(cfg, `model_provider = "magpie"`) {
		t.Fatalf("forced API login: magpie its provider, not beside a sign-in Codex won't use:\n%s", cfg)
	}
	if strings.Contains(cfg, "requires_openai_auth") {
		t.Fatalf("forced API login: no requires_openai_auth, or the app asks for a sign-in:\n%s", cfg)
	}
	if !strings.Contains(cfg, `forced_login_method = "api"`) {
		t.Fatalf("the user's forced_login_method kept:\n%s", cfg)
	}
	if c := cx.Check(); c != "" {
		t.Fatal(c)
	}

	// the user takes the line out to sign in to ChatGPT again
	p := filepath.Join(home, ".codex", "config.toml")
	if err := os.WriteFile(p, []byte(strings.Replace(cfg, "forced_login_method = \"api\"\n", "", 1)), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := cx.Sync(); err != nil {
		t.Fatal(err)
	}
	if cfg := read(); !strings.Contains(cfg, "requires_openai_auth = true") {
		t.Fatalf("line gone: the ChatGPT account back in the app (requires_openai_auth):\n%s", cfg)
	}

	// "chatgpt" forced, or none: the sign-in counts as before
	for _, c := range []string{"forced_login_method = \"chatgpt\"\n", ""} {
		home, _ := codexHome(t, `{"tokens":{"access_token":"x"}}`, c)
		if !codexChatGPT(filepath.Join(home, ".codex")) {
			t.Fatalf("config %q: a ChatGPT sign-in", c)
		}
	}
}
