package provider

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// Listing an agent's or a plugin's accounts with nothing to change looks
// at the accounts as kept: a copy of the whole list for each of a page's
// dozen lists was 61 MB of twenty window polls. A list with a change
// still makes its own copy and writes it.
func TestAccountListsCopyNoLoginsUnchanged(t *testing.T) {
	signIn(t)
	all := func(savedLogin) bool { return true }
	pp := plugin.Provider{ID: "fakeco", Accounts: []plugin.Account{{Key: "k1", AccountID: "one"}}}
	sideLogins("google", "a@example.com", all) // its own sign-in: written
	pluginLogins(pp)                           // its account: written
	before, locks := loginsCopied.Load(), pluginLoginLocks.Load()
	for range 20 {
		sideLogins("google", "a@example.com", all)
		if got := pluginLogins(pp); len(got) != 1 || got[0].User != "one" {
			t.Fatalf("plugin accounts: %+v", got)
		}
	}
	if n := loginsCopied.Load() - before; n != 0 {
		t.Fatalf("20 lists with nothing to change copied the accounts %d times", n)
	}
	// nor took logins.json's lock, a file opened across magpies
	if n := pluginLoginLocks.Load() - locks; n != 0 {
		t.Fatalf("20 plugin lists with nothing to change took the lock %d times", n)
	}
	locks = pluginLoginLocks.Load()

	pp.Accounts = append(pp.Accounts, plugin.Account{Key: "k2", AccountID: "two"})
	if got := pluginLogins(pp); len(got) != 2 {
		t.Fatalf("a second account wasn't listed: %+v", got)
	}
	pp.Accounts = pp.Accounts[1:]
	if got := pluginLogins(pp); len(got) != 1 || got[0].User != "two" {
		t.Fatalf("a signed-out account is still listed: %+v", got)
	}
	if n := pluginLoginLocks.Load() - locks; n != 2 {
		t.Fatalf("two changes took the lock %d times, want 2", n)
	}
	n := 0
	for _, l := range readLogins() {
		if l.Agent == "plugin:fakeco" {
			n++
		}
	}
	if n != 1 {
		t.Fatalf("logins.json holds %d of the plugin's accounts, want 1", n)
	}
}
