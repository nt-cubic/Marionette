package agent

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
)

// Codex in a WSL distro under NAT connected while the gateway wasn't shared
// was written gateway.Token, which the gateway turns away from beyond
// loopback; turning sharing on afterwards left it there, and every request
// was 401 "not an enabled magpie gateway key" (tried on Windows with
// magpie-desktop). Rekey, run as sharing changes, gives it the sharing key,
// and gateway.Token again once sharing is off.
func TestRekeyGivesWSLAgentTheSharingKey(t *testing.T) {
	t.Setenv("MAGPIE_ADDR", "")
	home, _ := codexHome(t, "", "model = \"gpt-5.5\"\n")
	probe := "home:/" + filepath.Base(home) + "\ndir:.codex\nroute:default via 172.23.80.1 dev eth0 proto kernel\nnet:nat\n"
	fakeWSL(t, "Ubuntu\r\n", "Ubuntu\r\n", map[string]string{"Ubuntu": probe},
		map[string]string{"Ubuntu": filepath.Dir(home)})
	ds := wslDistros()
	if len(ds) != 1 || ds[0].Mirrored {
		t.Fatalf("%+v", ds)
	}
	a := wslCodex(ds[0])
	if !strings.HasPrefix(a.Gateway(), "http://172.23.80.1:") {
		t.Fatalf("pointed at %s", a.Gateway())
	}
	if err := a.Fields[0].Set("fake/m1"); err != nil {
		t.Fatal(err)
	}
	cfg := filepath.Join(home, ".codex", "config.toml")
	token := func() string {
		t.Helper()
		tb, err := edit.GetTOMLTable(cfg, "model_providers."+magpieID)
		if err != nil {
			t.Fatal(err)
		}
		return tb["experimental_bearer_token"]
	}
	if k := token(); k != gateway.Token {
		t.Fatalf("connected unshared: key %q", k)
	}

	on := []*Agent{a}
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	lan := access.LANSecret()
	if lan == "" {
		t.Fatal("no sharing key")
	}
	moved, err := Rekey(on)
	if err != nil {
		t.Fatal(err)
	}
	if k := token(); k != lan {
		t.Fatalf("sharing on: key %q, want the sharing key (moved %v)", k, moved)
	}
	if !a.Wired() {
		t.Error("no longer connected")
	}
	if d := a.Drift(); d != nil {
		t.Errorf("drift %+v", d)
	}

	if err := access.ConfigureLAN(true, true); err != nil {
		t.Fatal(err)
	}
	if _, err := Rekey(on); err != nil {
		t.Fatal(err)
	}
	if k := token(); k == lan || k != access.LANSecret() {
		t.Fatalf("key made anew: config has %q", k)
	}

	if err := access.ConfigureLAN(false, false); err != nil {
		t.Fatal(err)
	}
	if _, err := Rekey(on); err != nil {
		t.Fatal(err)
	}
	if k := token(); k != gateway.Token {
		t.Fatalf("sharing off: key %q", k)
	}
}

// Rekey leaves an agent on this machine's loopback as it is: its key
// doesn't follow sharing.
func TestRekeyLeavesLoopback(t *testing.T) {
	t.Setenv("MAGPIE_ADDR", "")
	_, read := codexHome(t, "", "model = \"gpt-5.5\"\n")
	var as []*Agent
	for _, a := range All() {
		if a.ID == "codex" {
			as = append(as, a)
		}
	}
	if len(as) != 1 {
		t.Fatalf("codex: %d", len(as))
	}
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	if moved, err := Rekey(as); err != nil || len(moved) != 0 {
		t.Fatalf("moved %v, %v", moved, err)
	}
	if cfg := read(); strings.Contains(cfg, "magpie") {
		t.Fatalf("written:\n%s", cfg)
	}
}
