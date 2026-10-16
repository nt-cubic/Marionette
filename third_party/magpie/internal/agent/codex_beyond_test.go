package agent

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
)

// A Codex signed in to ChatGPT that reaches the gateway from another
// machine (a WSL distro under NAT; here MAGPIE_ADDR pinned beyond
// loopback, the same address for the gateway) isn't joined beside its
// sign-in: it would send its ChatGPT token to /backend-api/codex, and from
// there the gateway takes a named key alone, so every request was a 401
// (#2145, lazydao). It is connected as a Codex with no ChatGPT sign-in is:
// magpie as its provider, with the LAN sharing key. One of its own models
// picked then goes straight to OpenAI on its sign-in, off magpie.
func TestCodexBeyondLoopbackNotJoined(t *testing.T) {
	home, read := codexHome(t, `{"auth_mode":"chatgpt","tokens":{"access_token":"at","refresh_token":"rt","id_token":"x.e30.x","account_id":"acc"}}`, "model = \"gpt-5.1\"\n")
	t.Setenv("MAGPIE_ADDR", "172.23.80.1:3425")
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { access.ConfigureLAN(false, false) })
	lan := access.LANSecret()
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	cx := codex(home)
	c, err := cx.ConnectHow()
	if err != nil {
		t.Fatal(err)
	}
	if c.How == "joined" || cx.Joined() {
		t.Fatalf("joined beside the ChatGPT sign-in from beyond loopback: %+v\n%s", c, read())
	}
	cfg := read()
	if p, _ := edit.GetTOMLTop(cx.Path, "model_provider"); p != magpieID {
		t.Fatalf("model_provider %q, want magpie\n%s", p, cfg)
	}
	if k, _ := edit.GetTOMLTable(cx.Path, "model_providers."+magpieID); k["experimental_bearer_token"] != lan {
		t.Fatalf("provider key %q, want the LAN sharing key\n%s", k["experimental_bearer_token"], cfg)
	}
	if !strings.Contains(cfg, "172.23.80.1:3425/v1") {
		t.Fatalf("provider not at the gateway's address\n%s", cfg)
	}
	if err := cx.Fields[0].Set("gpt-5.1"); err != nil {
		t.Fatal(err)
	}
	if cfg := read(); strings.Contains(cfg, "model_provider =") || strings.Contains(cfg, "openai_base_url") {
		t.Fatalf("Codex's own model still through magpie:\n%s", cfg)
	}
}
