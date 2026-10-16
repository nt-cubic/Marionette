package backup

import (
	"encoding/json"
	"reflect"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

func TestCollectSettingsKeysCompatibility(t *testing.T) {
	home(t)
	appdir.UseExecutable("")
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	for _, keys := range []bool{false, true} {
		b, err := Collect(keys, "test")
		if err != nil {
			t.Fatal(err)
		}
		plain, err := json.Marshal(b)
		if err != nil {
			t.Fatal(err)
		}
		var fields map[string]json.RawMessage
		if err := json.Unmarshal(plain, &fields); err != nil {
			t.Fatal(err)
		}
		for _, field := range []string{"providersKeys", "settingsKeys", "libraryKeys"} {
			if _, ok := fields[field]; ok {
				t.Errorf("a whole backup added a sync-only %s marker", field)
			}
		}
		if b.Keys != keys {
			t.Error("a whole backup changed its existing credential policy")
		}
		if !keys && (b.GatewayKeys != nil || b.Settings.LANKey != "" || b.Settings.LANKeyID != "") {
			t.Error("a keyless backup included settings credentials")
		}
	}
}

func TestRestoreSettingsKeysScope(t *testing.T) {
	for _, tc := range []struct {
		name  string
		keys  bool
		parts Parts
	}{
		{"old-keyless", false, All},
		{"settings-only", true, Parts{Settings: true}},
		{"settings-excluded", true, Parts{Providers: true, Library: true}},
		{"all-parts", true, All},
	} {
		t.Run(tc.name, func(t *testing.T) {
			home(t)
			appdir.UseExecutable("")
			provider.ForgetAccounts()
			t.Cleanup(provider.ForgetAccounts)
			if err := access.ConfigureLAN(true, false); err != nil {
				t.Fatal(err)
			}
			cur := settings.Load()
			cur.Theme, cur.GitHubToken, cur.Proxy = "dark", "fixture-local-github", "direct"
			cur.Window = []int{900, 700}
			cur.OTel = settings.OTel{Endpoint: "https://local.example.com", Headers: map[string]string{"Authorization": "fixture-local-otel"}}
			if err := settings.Save(cur); err != nil {
				t.Fatal(err)
			}
			if err := provider.Save(provider.Provider{ID: "acme", Name: "Local", Chat: "https://local.example.com/v1", Key: "fixture-local-provider"}); err != nil {
				t.Fatal(err)
			}
			if _, err := library.SaveServer("", library.Server{Name: "github", Transport: "stdio", Command: "local-mcp",
				Env: map[string]string{"GITHUB_TOKEN": "fixture-local-mcp", "MODE": "local"}, Agents: []string{}}); err != nil {
				t.Fatal(err)
			}
			incoming := settings.Settings{Theme: "light", LAN: true, LANKeyID: "remote-lan", LANKey: "fixture-remote-lan",
				GitHubToken: "fixture-remote-github", Proxy: "http://remote.example.com", Window: []int{1, 2},
				OTel: settings.OTel{Endpoint: "https://remote.example.com", Headers: map[string]string{"Authorization": "fixture-remote-otel"}}}
			keys := []access.Key{{ID: incoming.LANKeyID, Name: "Remote", LAN: true, Secret: incoming.LANKey},
				{ID: "client", Name: "Client", Secret: "fixture-remote-client"}}
			version := 1
			if tc.keys {
				version = BundleVersion // a sync bundle from a keyed uploader carries the markers
			}
			b := Bundle{Version: version, SettingsKeys: Flag(tc.keys), Settings: &incoming, GatewayKeys: &keys,
				Providers: []provider.Provider{{ID: "acme", Name: "Remote", Chat: "https://remote.example.com/v1"}},
				Library: &library.Bundle{MCP: []*library.Server{{Name: "github", Transport: "stdio", Command: "remote-mcp",
					Env: map[string]string{"GITHUB_TOKEN": "", "MODE": "remote"}, Agents: []string{}}}}}
			r, err := Restore(b, tc.parts)
			if err != nil || r.Settings != tc.parts.Settings || r.Library != tc.parts.Library {
				t.Fatal("restore did not honor the selected parts", r, err)
			}
			got := settings.Load()
			wantTheme, wantGitHub, wantLAN, wantID, wantHeaders := cur.Theme, cur.GitHubToken, cur.LANKey, cur.LANKeyID, cur.OTel.Headers
			if tc.parts.Settings {
				wantTheme = incoming.Theme
				wantHeaders = nil // a keyless restore must not send headers to a different endpoint
				if tc.keys {
					wantGitHub, wantLAN, wantID, wantHeaders = incoming.GitHubToken, incoming.LANKey, incoming.LANKeyID, incoming.OTel.Headers
				}
			}
			if got.Theme != wantTheme || got.GitHubToken != wantGitHub || got.LANKey != wantLAN || got.LANKeyID != wantID || !reflect.DeepEqual(got.OTel.Headers, wantHeaders) {
				t.Error("settings credentials did not follow their selected credential scope")
			}
			if got.Proxy != cur.Proxy || !reflect.DeepEqual(got.Window, cur.Window) {
				t.Error("restoring scoped settings replaced this computer's own preferences")
			}
			_, acceptsRemote := access.Authenticate(keys[1].Secret)
			_, acceptsLocal := access.Authenticate(cur.LANKey)
			if acceptsRemote != (tc.keys && tc.parts.Settings) || acceptsLocal == (tc.keys && tc.parts.Settings) {
				t.Error("gateway-key replacement did not honor the settings-only credential scope")
			}
			p, err := provider.Find("acme")
			if err != nil || p.Key != "fixture-local-provider" {
				t.Error("the settings credential marker replaced a provider's local key", err)
			}
			lib, err := library.Collect()
			if err != nil || lib.MCP[0].Env["GITHUB_TOKEN"] != "fixture-local-mcp" {
				t.Error("the settings credential marker cleared a library's local token", err)
			}
		})
	}
}

func TestRestoreSettingsKeysStore(t *testing.T) {
	for _, store := range []string{"key-only", "empty", "legacy"} {
		t.Run(store, func(t *testing.T) {
			home(t)
			appdir.UseExecutable("")
			if err := access.ConfigureLAN(true, false); err != nil {
				t.Fatal(err)
			}
			cur := settings.Load()
			cur.Theme, cur.Proxy = "dark", "direct"
			if err := settings.Save(cur); err != nil {
				t.Fatal(err)
			}
			b := Bundle{Version: 1}
			switch store {
			case "key-only":
				b.GatewayKeys = &[]access.Key{{ID: "client", Name: "Client", Secret: "fixture-client"}}
			case "empty":
				b.GatewayKeys = &[]access.Key{}
			case "legacy":
				b.Settings = &settings.Settings{Theme: "light", LAN: true, LANKey: "fixture-older-lan", LANKeyID: "old-marker", GitHubToken: "fixture-older-github"}
			}
			b.SettingsKeys = Flag(true)
			if _, err := Restore(b, Parts{Settings: true}); err != nil {
				t.Fatal(err)
			}
			keys, err := access.Export()
			if err != nil {
				t.Fatal(err)
			}
			got := settings.Load()
			switch store {
			case "key-only":
				if !reflect.DeepEqual(keys, *b.GatewayKeys) || got.Theme != cur.Theme {
					t.Error("a settings-scoped key-only store did not replace keys and retain preferences")
				}
			case "empty":
				if len(keys) != 0 {
					t.Error("a settings-scoped explicit empty store did not revoke all keys")
				}
			case "legacy":
				if got.Theme != "light" || got.GitHubToken != "fixture-older-github" || got.LANKey != cur.LANKey || got.LANKeyID != cur.LANKeyID {
					t.Error("a settings-scoped older bundle lost credentials or detached the existing default key")
				}
			}
			if got.Proxy != cur.Proxy {
				t.Error("restoring a settings-scoped store changed this computer's proxy")
			}
		})
	}
}

func TestRestoreSettingsKeysLegacyLAN(t *testing.T) {
	home(t)
	appdir.UseExecutable("")
	b := Bundle{Version: BundleVersion, SettingsKeys: Flag(true), Settings: &settings.Settings{LAN: true, LANKey: "fixture-legacy-lan", LANKeyID: "incomplete-marker"}}
	if _, err := Restore(b, Parts{Settings: true}); err != nil {
		t.Fatal(err)
	}
	if _, ok := access.Authenticate(b.Settings.LANKey); !ok {
		t.Error("a settings-scoped legacy LAN credential was not migrated on a fresh machine")
	}
}
