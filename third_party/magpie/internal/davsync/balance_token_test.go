package davsync

import (
	"context"
	"fmt"
	"reflect"
	"testing"

	"github.com/yetone/magpie/internal/backup"
	"github.com/yetone/magpie/internal/provider"
)

// A computer sending no keys leaves the server's balance token in place,
// just as it leaves its API keys. Sending keys replaces the token too. A
// Volcengine access key (#1427) goes the same way, ID and Secret together.
func TestTakeBalanceToken(t *testing.T) {
	for _, c := range []struct {
		name                     string
		serverKeys, incomingKeys bool
		token, want              string
	}{
		{"keyless update", true, false, "", "balance-server"},
		{"explicit token", true, false, "balance-new", "balance-new"},
		{"with keys", true, true, "balance-new", "balance-new"},
		{"with keys without token", true, true, "", ""},
		{"keyless server", false, false, "", ""},
		{"adding keys", false, true, "balance-new", "balance-new"},
	} {
		t.Run(c.name, func(t *testing.T) {
			server := provider.Provider{ID: "relay", Name: "Old", Chat: "https://old.example.com/v1"}
			if c.serverKeys {
				server.Key, server.BalanceToken = "sk-server", "balance-server"
				server.AccessKeyID, server.SecretAccessKey = "AK-server", "SK-server"
			}
			incoming := provider.Provider{ID: "relay", Name: "New", Chat: "https://new.example.com/v1", BalanceToken: c.token}
			if c.incomingKeys {
				incoming.Key = "sk-new"
				incoming.AccessKeyID, incoming.SecretAccessKey = "AK-new", "SK-new"
			}
			to := backup.Bundle{Keys: c.serverKeys, Providers: []provider.Provider{server}}
			from := backup.Bundle{Keys: c.incomingKeys, Providers: []provider.Provider{incoming}}
			take(&to, from, "providers")
			if len(to.Providers) != 1 {
				t.Fatalf("providers: %+v", to.Providers)
			}
			got := to.Providers[0]
			if got.BalanceToken != c.want || got.Name != incoming.Name || got.Chat != incoming.Chat {
				t.Fatalf("merged: %+v, want balance token %q and the incoming name and URL", got, c.want)
			}
			wantKey, wantAK, wantSK := incoming.Key, incoming.AccessKeyID, incoming.SecretAccessKey
			if c.serverKeys && !c.incomingKeys {
				wantKey, wantAK, wantSK = server.Key, server.AccessKeyID, server.SecretAccessKey
			}
			if got.AccessKeyID != wantAK || got.SecretAccessKey != wantSK {
				t.Fatalf("access key: %q %q, want %q %q", got.AccessKeyID, got.SecretAccessKey, wantAK, wantSK)
			}
			if got.Key != wantKey || to.Keys != (c.serverKeys || c.incomingKeys) || (to.Keys || (to.ProvidersKeys != nil && *to.ProvidersKeys)) != (c.serverKeys || c.incomingKeys) {
				t.Fatalf("keys after merge: %+v", to)
			}
			if !reflect.DeepEqual(from.Providers, []provider.Provider{incoming}) {
				t.Fatalf("merge changed the incoming bundle: %+v", from.Providers)
			}
		})
	}
}

// Exercise the encrypted remote and two separate computers. Keyless
// uploads keep each computer's token, and any token already on the server.
func TestSyncBalanceToken(t *testing.T) {
	for _, serverKeys := range []bool{false, true} {
		t.Run(fmt.Sprintf("serverKeys=%v", serverKeys), func(t *testing.T) {
			f, srv := newFakeS3(t)
			cfg := f.config(srv)
			cfg.Keys, cfg.Agents = serverKeys, false
			a, b := newComputer(t), newComputer(t)
			use := func(c computer) {
				c.use(t)
				t.Setenv("USERPROFILE", string(c))
			}
			now := func() {
				t.Helper()
				if err := Now(context.Background()); err != nil {
					t.Fatal(err)
				}
			}
			remote := func() backup.Bundle {
				t.Helper()
				got, err := backup.Open(f.objects["team x+y/magpie/magpie.magpie-backup"], cfg.Passphrase)
				if err != nil {
					t.Fatal(err)
				}
				return got
			}
			use(a)
			if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Chat: "https://relay.example.com/v1",
				Key: "sk-a", BalanceToken: "balance-a", AccessKeyID: "AK-a", SecretAccessKey: "SK-a"}); err != nil {
				t.Fatal(err)
			}
			if err := Configure(cfg); err != nil {
				t.Fatal(err)
			}
			now()
			wantKey, wantToken, wantSecret := "", "", ""
			if serverKeys {
				wantKey, wantToken, wantSecret = "sk-a", "balance-a", "SK-a"
			}
			if got := remote(); got.Keys != serverKeys || len(got.Providers) != 1 || got.Providers[0].Key != wantKey || got.Providers[0].BalanceToken != wantToken || got.Providers[0].SecretAccessKey != wantSecret {
				t.Fatalf("first upload: %+v", got)
			}

			use(b)
			if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay b", Chat: "https://relay.example.com/v1",
				Key: "sk-b", BalanceToken: "balance-b", AccessKeyID: "AK-b", SecretAccessKey: "SK-b"}); err != nil {
				t.Fatal(err)
			}
			keyless := cfg
			keyless.Keys = false
			if err := Configure(keyless); err != nil {
				t.Fatal(err)
			}
			now()
			localKey, localToken, localSecret := "sk-b", "balance-b", "SK-b"
			if serverKeys {
				localKey, localToken, localSecret = "sk-a", "balance-a", "SK-a"
			}
			ps, _ := provider.Stored()
			if len(ps) != 1 || ps[0].Key != localKey || ps[0].BalanceToken != localToken || ps[0].SecretAccessKey != localSecret {
				t.Fatalf("b after download: %+v", ps)
			}
			p := ps[0]
			p.Name, p.BalanceToken = "Renamed", "balance-b-new"
			if err := provider.Save(p); err != nil {
				t.Fatal(err)
			}
			now()
			if got := remote(); got.Keys != serverKeys || len(got.Providers) != 1 || got.Providers[0].Name != "Renamed" || got.Providers[0].Key != wantKey || got.Providers[0].BalanceToken != wantToken || got.Providers[0].SecretAccessKey != wantSecret {
				t.Fatalf("after b's keyless upload: %+v", got)
			}
			if ps, _ := provider.Stored(); len(ps) != 1 || ps[0].BalanceToken != "balance-b-new" {
				t.Fatalf("upload changed b's token: %+v", ps)
			}
			use(a)
			now()
			if ps, _ := provider.Stored(); len(ps) != 1 || ps[0].Name != "Renamed" || ps[0].Key != "sk-a" || ps[0].BalanceToken != "balance-a" || ps[0].AccessKeyID != "AK-a" || ps[0].SecretAccessKey != "SK-a" {
				t.Fatalf("a after download: %+v", ps)
			}
		})
	}
}
