package davsync

import (
	"context"
	"encoding/json"
	"reflect"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/library"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// Adding gateway keys to a keyless backup must not make a later library
// download treat its redacted MCP tokens as credentials to clear here.
func TestGatewayKeysPreserveLibrarySecretsSync(t *testing.T) {
	f, srv := newFakeS3(t)
	cfg := f.config(srv)
	cfg.Keys, cfg.Agents = false, false
	a, b := newComputer(t), newComputer(t)
	use := func(c computer) {
		t.Helper()
		c.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	puts := func() int {
		f.mu.Lock()
		defer f.mu.Unlock()
		n := 0
		for _, request := range f.log {
			if strings.HasPrefix(request, "PUT") {
				n++
			}
		}
		return n
	}
	remote := func() backup.Bundle {
		t.Helper()
		f.mu.Lock()
		data := slices.Clone(f.objects["team x+y/magpie/magpie.magpie-backup"])
		f.mu.Unlock()
		got, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return got
	}
	say := func(text string) {
		t.Helper()
		if _, err := library.SaveInstructions(library.InstructionsChange{Shared: &text}); err != nil {
			t.Fatal(err)
		}
	}
	server := func(token string) {
		t.Helper()
		if _, err := library.SaveServer("", library.Server{Name: "github", Transport: "stdio", Command: "fixture-mcp",
			Env: map[string]string{"GITHUB_TOKEN": token, "MODE": "ro"}, Agents: []string{}}); err != nil {
			t.Fatal(err)
		}
	}

	use(a)
	say("Initial instructions.")
	server("")
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	initial := remote()
	if initial.Keys || initial.Library == nil || len(initial.Library.MCP) != 1 || initial.Library.MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("the initial keyless library was not redacted")
	}

	use(b)
	server("fixture-local-mcp-token")
	old, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	who, ok := access.Authenticate(old)
	if !ok {
		t.Fatal("the local gateway credential is invalid")
	}
	withKeys := cfg
	withKeys.Keys = true
	if err := Configure(withKeys); err != nil {
		t.Fatal(err)
	}
	now()
	here, err := library.Collect()
	if err != nil || here.MCP[0].Env["GITHUB_TOKEN"] != "fixture-local-mcp-token" || here.Texts["default"] != "Initial instructions." {
		t.Fatal("joining the keyless library did not retain the local MCP token", err)
	}
	before := puts()
	now()
	if puts() != before {
		t.Fatal("joining uploaded an unchanged library")
	}

	prefs := settings.Load()
	next, err := access.Update("rotate-key", access.Change{Key: who.KeyID})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(settings.Load(), prefs) {
		t.Fatal("rotating the independent key changed preferences")
	}
	now()
	got := remote()
	keys, err := access.Export()
	if err != nil || puts() != before+1 || got.GatewayKeys == nil || !reflect.DeepEqual(*got.GatewayKeys, keys) {
		t.Fatal("the independent gateway-key rotation was not uploaded once", err)
	}
	t.Logf("after gateway-key-only upload: Keys=%v, library token present=%v", got.Keys, got.Library.MCP[0].Env["GITHUB_TOKEN"] != "")

	use(a)
	now()
	if _, ok := access.Authenticate(next); !ok {
		t.Fatal("computer A did not receive the rotated gateway credential")
	}
	say("Updated instructions.")
	now()
	if puts() != before+2 {
		t.Fatal("computer A's instruction change was not uploaded once")
	}
	use(b)
	now()
	here, err = library.Collect()
	if err != nil || here.Texts["default"] != "Updated instructions." {
		t.Fatal("computer B did not receive the updated instructions", err)
	}
	if here.MCP[0].Env["GITHUB_TOKEN"] != "fixture-local-mcp-token" {
		t.Error("a gateway-key-only upload caused a later library download to clear computer B's MCP token")
	}
	now()
	if puts() != before+2 {
		t.Error("the received library was uploaded again without a local change")
	}
}

// A computer leaving the library out must still sync its gateway keys without
// exporting its local library or erasing a later client's private MCP token.
func TestGatewayKeysWithLibraryOffSync(t *testing.T) {
	f, srv := newFakeS3(t)
	cfg := f.config(srv)
	cfg.Keys, cfg.Agents = false, false
	a, b, c := newComputer(t), newComputer(t), newComputer(t)
	use := func(c computer) {
		t.Helper()
		c.use(t)
		appdir.UseExecutable("")
		provider.ForgetAccounts()
	}
	t.Cleanup(provider.ForgetAccounts)
	now := func() {
		t.Helper()
		if err := Now(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	remote := func() backup.Bundle {
		t.Helper()
		f.mu.Lock()
		data := slices.Clone(f.objects["team x+y/magpie/magpie.magpie-backup"])
		f.mu.Unlock()
		got, err := backup.Open(data, cfg.Passphrase)
		if err != nil {
			t.Fatal(err)
		}
		return got
	}
	say := func(text string) {
		t.Helper()
		if _, err := library.SaveInstructions(library.InstructionsChange{Shared: &text}); err != nil {
			t.Fatal(err)
		}
	}
	server := func(token string) {
		t.Helper()
		if _, err := library.SaveServer("", library.Server{Name: "github", Transport: "stdio", Command: "fixture-mcp",
			Env: map[string]string{"GITHUB_TOKEN": token}, Agents: []string{}}); err != nil {
			t.Fatal(err)
		}
	}

	use(a)
	say("Shared instructions.")
	server("fixture-a-mcp-token")
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	original := remote()
	if original.Keys || original.Library.MCP[0].Env["GITHUB_TOKEN"] != "" {
		t.Fatal("computer A uploaded a private token with Keys:false")
	}
	use(b)
	say("Private instructions.")
	server("fixture-b-private-mcp-token")
	if _, err := library.SaveServer("", library.Server{Name: "private", Transport: "stdio", Command: "private-mcp",
		Env: map[string]string{"API_KEY": "fixture-private-server-key"}, Agents: []string{}}); err != nil {
		t.Fatal(err)
	}
	old, err := access.Update("add-key", access.Change{Name: "Laptop"})
	if err != nil {
		t.Fatal(err)
	}
	who, ok := access.Authenticate(old)
	if !ok {
		t.Fatal("the independent gateway credential is invalid")
	}
	off := false
	withoutLibrary := cfg
	withoutLibrary.Keys, withoutLibrary.Library = true, &off
	if err := Configure(withoutLibrary); err != nil {
		t.Fatal(err)
	}
	now()
	next, err := access.Update("rotate-key", access.Change{Key: who.KeyID})
	if err != nil {
		t.Fatal(err)
	}
	now()
	got := remote()
	if !reflect.DeepEqual(got.Library, original.Library) {
		t.Error("a gateway-key upload exported a library excluded from sync")
	}
	plain, err := json.Marshal(got)
	if err != nil {
		t.Fatal(err)
	}
	if got.Keys || strings.Contains(string(plain), "fixture-b-private-") || strings.Contains(string(plain), "fixture-private-server-") {
		t.Error("a settings-only upload authorized or exposed excluded library secrets")
	}
	if got.GatewayKeys == nil || len(*got.GatewayKeys) != 1 || (*got.GatewayKeys)[0].Secret != next {
		t.Fatal("the gateway-key-only upload was not stored")
	}
	use(a)
	now()
	if _, ok := access.Authenticate(next); !ok {
		t.Fatal("computer A did not receive the gateway-key rotation")
	}
	say("Updated shared instructions.")
	now()
	use(c)
	server("fixture-c-private-mcp-token")
	if err := Configure(cfg); err != nil {
		t.Fatal(err)
	}
	now()
	here, err := library.Collect()
	if err != nil || here.Texts["default"] != "Updated shared instructions." {
		t.Fatal("the later client did not receive the shared library", err)
	}
	if here.MCP[0].Env["GITHUB_TOKEN"] != "fixture-c-private-mcp-token" {
		t.Error("uploading gateway keys with Library:false cleared the later client's MCP token")
	}
	if _, ok := access.Authenticate(next); !ok {
		t.Error("the later client did not receive settings credentials")
	}
}

func TestGatewayKeysTakeScopedSettings(t *testing.T) {
	for _, tc := range []struct {
		name                           string
		serverKeys, sourceKeys, scoped bool
		store                          string
	}{
		{"full-source", false, true, false, "replace"},
		{"scoped-source", false, false, true, "replace"},
		{"scoped-empty", false, false, true, "empty"},
		{"scoped-legacy", false, false, true, "legacy"},
		{"scoped-key-only", false, false, true, "key-only"},
		{"full-server", true, false, true, "replace"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			old := settings.Settings{Theme: "dark", LANKey: "fixture-old-lan", LANKeyID: "old-lan", GitHubToken: "fixture-old-github"}
			oldKeys := []access.Key{{ID: "old", Name: "Old", Secret: "fixture-old-gateway"}}
			lib := &library.Bundle{MCP: []*library.Server{{Name: "github", Transport: "stdio", Command: "remote-mcp", Env: map[string]string{"GITHUB_TOKEN": ""}}}}
			to := backup.Bundle{Keys: tc.serverKeys, Settings: &old, GatewayKeys: &oldKeys, Library: lib,
				Providers: []provider.Provider{{ID: "acme", Name: "Acme", Chat: "https://acme.example.com/v1"}}}
			next := settings.Settings{Theme: "light", LANKey: "fixture-new-lan", LANKeyID: "new-lan", GitHubToken: "fixture-new-github"}
			from := backup.Bundle{Keys: tc.sourceKeys, Settings: &next, GatewayKeys: &[]access.Key{{ID: "new", Name: "New", Secret: "fixture-new-gateway"}},
				Library:   &library.Bundle{MCP: []*library.Server{{Name: "private", Env: map[string]string{"TOKEN": "fixture-unselected-library"}}}},
				Providers: []provider.Provider{{ID: "private", Key: "fixture-unselected-provider"}}}
			switch tc.store {
			case "empty":
				from.GatewayKeys = &[]access.Key{}
			case "legacy":
				from.GatewayKeys = nil
			case "key-only":
				from.Settings = nil
			}
			from.SettingsKeys = backup.Flag(tc.scoped)
			beforeTo, _ := json.Marshal(to)
			beforeFrom, _ := json.Marshal(from)
			merged := to
			take(&merged, from, "settings")
			if merged.Keys != tc.serverKeys {
				t.Error("taking settings changed the credential policy of unselected parts")
			}
			wantSettings, wantKeys := from.Settings, from.GatewayKeys
			if wantSettings == nil {
				wantSettings = to.Settings
			}
			if wantKeys == nil {
				wantKeys = to.GatewayKeys
			}
			if !reflect.DeepEqual(merged.Settings, wantSettings) || !reflect.DeepEqual(merged.GatewayKeys, wantKeys) {
				t.Error("taking settings did not carry its authorized credentials or preserve missing fields")
			}
			if merged.Library != to.Library || !reflect.DeepEqual(merged.Providers, to.Providers) {
				t.Error("taking settings changed an unselected library or provider")
			}
			plain, err := json.Marshal(merged)
			if err != nil {
				t.Fatal(err)
			}
			var marker struct {
				SettingsKeys bool `json:"settingsKeys"`
			}
			if err := json.Unmarshal(plain, &marker); err != nil || !marker.SettingsKeys {
				t.Error("the merged settings did not retain their own credential marker", err)
			}
			if strings.Contains(string(plain), "fixture-unselected-") {
				t.Error("taking settings carried a credential from an unselected part")
			}
			afterTo, _ := json.Marshal(to)
			afterFrom, _ := json.Marshal(from)
			if !slices.Equal(beforeTo, afterTo) || !slices.Equal(beforeFrom, afterFrom) {
				t.Error("taking settings mutated an input bundle")
			}
		})
	}
}

func TestGatewayKeysTakeKeylessOntoScopedSettings(t *testing.T) {
	for _, sameEndpoint := range []bool{false, true} {
		t.Run(map[bool]string{false: "different-endpoint", true: "same-endpoint"}[sameEndpoint], func(t *testing.T) {
			old := settings.Settings{Theme: "dark", LANKey: "fixture-server-lan", LANKeyID: "server-lan", GitHubToken: "fixture-server-github",
				OTel: settings.OTel{Endpoint: "https://collector.example.com", Headers: map[string]string{"Authorization": "fixture-server-otel"}}}
			to := backup.Bundle{Version: backup.BundleVersion, SettingsKeys: backup.Flag(true), Settings: &old,
				GatewayKeys: &[]access.Key{{ID: "server", Name: "Server", Secret: "fixture-server-gateway"}}}
			next := settings.Settings{Theme: "light", OTel: settings.OTel{Endpoint: "https://other.example.com"}}
			if sameEndpoint {
				next.OTel.Endpoint = old.OTel.Endpoint + "/"
			}
			from := backup.Bundle{Settings: &next, GatewayKeys: &[]access.Key{{ID: "private", Secret: "fixture-unauthorized-gateway"}}}
			before, _ := json.Marshal(from)
			take(&to, from, "settings")
			if to.Keys || to.Settings.Theme != "light" || to.Settings.LANKey != old.LANKey || to.Settings.LANKeyID != old.LANKeyID || to.Settings.GitHubToken != old.GitHubToken {
				t.Error("a keyless upload erased scoped settings credentials or changed the global policy")
			}
			wantHeaders := map[string]string(nil)
			if sameEndpoint {
				wantHeaders = old.OTel.Headers
			}
			if !reflect.DeepEqual(to.Settings.OTel.Headers, wantHeaders) || (*to.GatewayKeys)[0].Secret != "fixture-server-gateway" {
				t.Error("a keyless update lost server credentials or moved headers to another endpoint")
			}
			after, _ := json.Marshal(from)
			if !slices.Equal(before, after) {
				t.Error("preserving server credentials mutated the keyless source")
			}
		})
	}
}
