package provider

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// #4416: a plugin keeps the vendor's id as accountId, the key the user's
// own name, proxy and gateway key are kept by (codearts: an IAM id), and
// tells the account's name in usage().user (a masked phone). The account
// is shown by the user's own name, else the plugin's, else its id: what
// was read last, kept across a restart and a failed read, never nothing.
func TestPluginAccountShownByItsUsageUser(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	// a month's window of no model's own, so the reading is kept on disk
	// as quotas.json keeps any (a pool of some models' isn't)
	ws := filepath.Join(t.TempDir(), "windows.json")
	if err := os.WriteFile(ws, []byte(`{"iam::01a18be16b297c5197d8569e4be6a479:user:hw059194106570983": [{"name": "Month", "used": 10, "span": 2592000}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("FAKE_WINDOWS", ws)
	t.Cleanup(plugin.Settle)
	forget := func() {
		loginUsageCache.Lock()
		loginUsageCache.m, loginUsageCache.pending = nil, nil
		loginUsageCache.Unlock()
		lastQuotas.Lock()
		lastQuotas.m, lastQuotas.loaded = nil, false
		lastQuotas.Unlock()
	}
	forget()
	t.Cleanup(forget)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	const iam, phone = "iam::01a18be16b297c5197d8569e4be6a479:user:hw059194106570983", "180****2361"
	k, err := plugin.Import(ctx, "fakeco", map[string]any{"type": "oauth", "refresh": "r", "access": "a", "expires": 9e15, "accountId": iam})
	if err != nil {
		t.Fatal(err)
	}
	saveLogins(t, savedLogin{Agent: "plugin:fakeco", User: iam, Home: k, On: true})
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	id := PluginID("fakeco")
	shown := func(want, why string) {
		t.Helper()
		p, err := Find(id)
		if err != nil {
			t.Fatal(err)
		}
		if got := p.AccountShown(iam); got != want {
			t.Fatalf("%s: the provider shows %q, not %q", why, got, want)
		}
		for _, of := range []string{id, "plugin:fakeco"} {
			if got := AccountShownOf(of, iam); got != want {
				t.Fatalf("%s: %s shows %q, not %q", why, of, got, want)
			}
		}
		var keys []string
		for _, v := range AccountNames() {
			if strings.HasPrefix(v, id+"/") {
				keys = append(keys, v)
			}
		}
		if len(keys) != 1 || keys[0] != id+"/"+want {
			t.Fatalf("%s: a gateway key's accounts read %q", why, keys)
		}
		alias := withAccountNames([]SubscriptionQuota{{Provider: id, User: iam}})[0].Alias
		if want == iam {
			want = ""
		}
		if alias != want {
			t.Fatalf("%s: the card's alias is %q, not %q", why, alias, want)
		}
	}
	shown(iam, "nothing read yet")
	if got := LoginUsage(ctx, "plugin:fakeco")[iam]; got.Error != "" || got.User != phone {
		t.Fatalf("the reading: %+v", got)
	}
	shown(phone, "read")
	// a restart: what quotas.json kept
	forget()
	shown(phone, "after a restart")
	// a read that failed holds the account's own id, not the phone
	loginUsageCache.Lock()
	loginUsageCache.m = map[string]loginUsageEntry{"plugin:fakeco/" + iam: {at: time.Now(), q: SubscriptionQuota{Provider: id, User: iam, Error: "fetch failed"}}}
	loginUsageCache.Unlock()
	shown(phone, "a failed read")
	// the user's own name is never overridden
	if err := SetAccountName(id, iam, "Work"); err != nil {
		t.Fatal(err)
	}
	shown("Work", "named")
	if err := SetAccountName(id, iam, ""); err != nil {
		t.Fatal(err)
	}
	// nothing known: the id, never blank
	lastQuotas.Lock()
	lastQuotas.m, lastQuotas.loaded = nil, true
	lastQuotas.Unlock()
	shown(iam, "nothing known")
}
