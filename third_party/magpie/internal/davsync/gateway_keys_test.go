package davsync

import (
	"context"
	"net/http/httptest"
	"reflect"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// Gateway keys travel with Settings in a backup. Changing just an independent
// key must sync too; a later theme change must not restore its old credential.
func TestGatewayKeysSync(t *testing.T) {
	for _, tc := range []struct {
		name, action string
		withTheme    bool
		lastKey      bool
	}{
		{name: "theme-control"},
		{name: "rotate-only", action: "rotate-key"},
		{name: "disable-only", action: "off-key"},
		{name: "remove-only", action: "remove-key"},
		{name: "disable-with-theme", action: "off-key", withTheme: true},
		{name: "remove-last-key", action: "remove-key", lastKey: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			fake := &fakeDAV{files: map[string][]byte{}, etags: map[string]string{}, dirs: map[string]bool{"/dav": true}, cond: true, putETag: true}
			srv := httptest.NewServer(fake)
			defer srv.Close()
			countPuts := func() int {
				fake.mu.Lock()
				defer fake.mu.Unlock()
				return fake.puts
			}
			cfg := Config{URL: srv.URL + "/dav/", User: "me", Password: "pw", Passphrase: "gateway sync fixture passphrase", Keys: true}
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
			remote := func() backup.Bundle {
				t.Helper()
				fake.mu.Lock()
				data := append([]byte(nil), fake.files["/dav/magpie/magpie.magpie-backup"]...)
				fake.mu.Unlock()
				got, err := backup.Open(data, cfg.Passphrase)
				if err != nil {
					t.Fatal(err)
				}
				return got
			}
			setTheme := func(theme string) {
				t.Helper()
				s := settings.Load()
				s.Theme = theme
				if err := settings.Save(s); err != nil {
					t.Fatal(err)
				}
			}

			use(a)
			setTheme("dark")
			if !tc.lastKey {
				if err := access.ConfigureLAN(true, false); err != nil {
					t.Fatal(err)
				}
			}
			old, err := access.Update("add-key", access.Change{Name: "Fixture laptop"})
			if err != nil {
				t.Fatal(err)
			}
			who, ok := access.Authenticate(old)
			if !ok || who.KeyID == settings.Load().LANKeyID {
				t.Fatal("fixture is not an enabled independent gateway key")
			}
			initial, err := access.Export()
			if err != nil {
				t.Fatal(err)
			}
			if err := Configure(cfg); err != nil {
				t.Fatal(err)
			}
			now()
			if got := remote(); got.GatewayKeys == nil || !reflect.DeepEqual(*got.GatewayKeys, initial) {
				t.Fatal("initial encrypted upload did not carry the gateway-key store")
			}
			use(b)
			if err := Configure(cfg); err != nil {
				t.Fatal(err)
			}
			now()
			if got, ok := access.Authenticate(old); !ok || got.KeyID != who.KeyID || settings.Load().Theme != "dark" {
				t.Fatal("initial sync did not restore the theme and independent key on computer B")
			}
			puts := countPuts()
			now()
			if countPuts() != puts {
				t.Fatal("an unchanged computer uploaded the setup again")
			}
			t.Log("initial encrypted sync and unchanged-sync controls pass")

			use(a)
			if tc.action == "" {
				setTheme("light")
				now()
				use(b)
				now()
				if _, ok := access.Authenticate(old); !ok || settings.Load().Theme != "light" || countPuts() != puts+1 {
					t.Fatal("theme-only control failed to sync without changing the gateway key")
				}
				t.Log("theme-only control uploads once and retains the key")
				return
			}

			before := settings.Load()
			next, err := access.Update(tc.action, access.Change{Key: who.KeyID})
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(settings.Load(), before) {
				t.Fatal("independent-key change unexpectedly modified settings.json")
			}
			if _, ok := access.Authenticate(old); ok {
				t.Fatal("local key operation did not revoke the old credential")
			}
			if next != "" {
				if got, ok := access.Authenticate(next); !ok || got.KeyID != who.KeyID {
					t.Fatal("local rotation did not preserve an enabled key's identity")
				}
			}
			want, err := access.Export()
			if err != nil {
				t.Fatal(err)
			}
			t.Log("local key operation revoked the credential without changing settings")
			if tc.withTheme {
				setTheme("light")
			}
			now()
			if countPuts() != puts+1 {
				t.Errorf("%s uploaded %d times, want 1; Now returned nil", tc.action, countPuts()-puts)
			}
			if got := remote(); got.GatewayKeys == nil || !reflect.DeepEqual(*got.GatewayKeys, want) {
				t.Error("the server backup retained the old enabled gateway credential")
			}
			if tc.withTheme && countPuts() == puts+1 {
				t.Log("the accompanying theme change uploaded; key propagation is checked independently")
			}
			use(b)
			now()
			if _, ok := access.Authenticate(old); ok {
				t.Error("computer B still accepts the revoked credential after sync")
			}
			if next != "" {
				if _, ok := access.Authenticate(next); !ok {
					t.Error("computer B does not accept the rotated credential after sync")
				}
			}

			// B changes only its palette after syncing; its next upload must not
			// resurrect A's revoked credential when A brings in that palette.
			theme := "light"
			if settings.Load().Theme == theme {
				theme = "dark"
			}
			puts = countPuts()
			setTheme(theme)
			now()
			if countPuts() != puts+1 {
				t.Fatal("computer B's unrelated theme change was not uploaded")
			}
			use(a)
			now()
			if settings.Load().Theme != theme {
				t.Fatal("computer A did not receive computer B's unrelated theme change")
			}
			t.Log("computer B uploaded its theme once and computer A received it")
			if _, ok := access.Authenticate(old); ok {
				t.Error("an unrelated theme sync resurrected the old credential on computer A")
			}
			if next != "" {
				if _, ok := access.Authenticate(next); !ok {
					t.Error("an unrelated theme sync invalidated the rotated credential on computer A")
				}
			}
		})
	}
}
