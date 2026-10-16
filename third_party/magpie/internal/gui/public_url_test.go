package gui

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/fx"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

// Settings shows the local address agents use; the Gateway page advertises
// the public URL for its copy buttons and snippets.
func TestPublicURLInConsole(t *testing.T) {
	h := t.TempDir()
	testenv.SetHome(t, h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(h, ".local", "share"))
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3425")
	t.Setenv("MAGPIE_PUBLIC_URL", "https://magpie.example.com/magpie///")

	// Seed the rate the Settings page reads, so it needs no network.
	cache := fx.CachePath()
	if err := os.MkdirAll(filepath.Dir(cache), 0o755); err != nil {
		t.Fatal(err)
	}
	b, err := json.Marshal(fx.Rate{CNYPerUSD: 7.25, At: time.Now()})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(cache, b, 0o644); err != nil {
		t.Fatal(err)
	}
	fx.Reset()
	t.Cleanup(fx.Reset)

	if err := settings.Save(settings.Settings{LAN: true, LANKey: "sk-magpie-test"}); err != nil {
		t.Fatal(err)
	}
	want := "https://magpie.example.com/magpie"
	if got := settingsState().Gateway; got != gateway.URL() {
		t.Errorf("Settings: %q, want %q", got, gateway.URL())
	}
	g := providersState().Gateway
	if g.URL != gateway.URL() {
		t.Errorf("Gateway address: %q, want the local %q", g.URL, gateway.URL())
	}
	if len(g.LANURLs) != 1 || g.LANURLs[0] != want {
		t.Errorf("LAN addresses: %v, want [%s]", g.LANURLs, want)
	}
	if !g.OnNetwork {
		t.Error("MAGPIE_ADDR 0.0.0.0 not said to be on the network")
	}
	// #1551: Connect starts at the public URL, and Settings shows it
	if g.PublicURL != want {
		t.Errorf("Gateway page's public URL: %q, want %q", g.PublicURL, want)
	}
	if got := settingsState().PublicURL; got != want {
		t.Errorf("Settings' public URL: %q, want %q", got, want)
	}
	b, err = json.Marshal(g)
	if err != nil {
		t.Fatal(err)
	}
	var state map[string]any
	if err := json.Unmarshal(b, &state); err != nil {
		t.Fatal(err)
	}
	if state["lan"] != true {
		t.Fatalf("LAN state missing from gateway page: %s", b)
	}
}

// A gateway MAGPIE_ADDR puts on the network (the Docker image) asks other
// machines for a gateway key whether or not it is shared, so the Gateway
// page shows its keys and offers no key-less "this computer" then.
func TestGatewayOnNetworkInConsole(t *testing.T) {
	h := t.TempDir()
	testenv.SetHome(t, h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(h, ".local", "share"))
	t.Setenv("MAGPIE_ADDR", "0.0.0.0:3425")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	if err := settings.Save(settings.Settings{LAN: false}); err != nil {
		t.Fatal(err)
	}
	if g := providersState().Gateway; !g.OnNetwork || !g.LAN {
		t.Errorf("gateway on the network, not shared, shown without its keys: %+v", g)
	}
	if err := settings.Save(settings.Settings{LAN: true}); err != nil {
		t.Fatal(err)
	}
	if g := providersState().Gateway; !g.OnNetwork || !g.LAN {
		t.Errorf("gateway on the network, shared: %+v", g)
	}
	t.Setenv("MAGPIE_ADDR", "127.0.0.1:3425")
	if err := settings.Save(settings.Settings{LAN: false}); err != nil {
		t.Fatal(err)
	}
	if g := providersState().Gateway; g.OnNetwork || g.LAN {
		t.Errorf("gateway on loopback, not shared: %+v", g)
	}
}

// #1551: MAGPIE_PUBLIC_URL is offered where other machines are let in,
// and nowhere else: on loopback, not shared, nothing reaches the gateway
// through it without a key it can't take.
func TestPublicURLOnlyWhereOffered(t *testing.T) {
	h := t.TempDir()
	testenv.SetHome(t, h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(h, ".local", "share"))
	t.Setenv("MAGPIE_PUBLIC_URL", "https://magpie.example.com")
	t.Setenv("MAGPIE_ADDR", "127.0.0.1:3425")
	if err := settings.Save(settings.Settings{LAN: false}); err != nil {
		t.Fatal(err)
	}
	if g := providersState().Gateway; g.PublicURL != "" {
		t.Errorf("loopback, not shared: Gateway page offers %q", g.PublicURL)
	}
	if u := settingsState().PublicURL; u != "" {
		t.Errorf("loopback, not shared: Settings shows %q", u)
	}
	if err := settings.Save(settings.Settings{LAN: true}); err != nil {
		t.Fatal(err)
	}
	if g := providersState().Gateway; g.PublicURL != "https://magpie.example.com" {
		t.Errorf("shared: Gateway page's public URL %q", g.PublicURL)
	}
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	if g := providersState().Gateway; g.PublicURL != "" {
		t.Errorf("no MAGPIE_PUBLIC_URL: Gateway page's public URL %q", g.PublicURL)
	}
	if u := settingsState().PublicURL; u != "" {
		t.Errorf("no MAGPIE_PUBLIC_URL: Settings shows %q", u)
	}
}
