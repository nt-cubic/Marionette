package provider

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// How many of a provider's accounts or keys take requests at once
// (01huadalang on Discord: ZCode bans an address three of its accounts are
// used from at once): the user's own MaxAccounts, 0 for no limit, over
// what the plugin giving the provider says, which goes for nil; none for a
// provider no plugin gives. The save keeps it in range and keeps it when
// set, and SetAtOnce refuses one out of range.
func TestAtOnce(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	plug := Provider{ID: "zc", Account: &Account{plugin: &plugin.Provider{ID: "zc", MaxAccounts: 2}}}
	if n := plug.AtOnce(); n != 2 {
		t.Fatalf("the plugin's own = %d, want 2", n)
	}
	if n := plug.PluginAtOnce(); n != 2 {
		t.Fatalf("PluginAtOnce = %d, want 2", n)
	}
	three, none := 3, 0
	plug.MaxAccounts = &three
	if n := plug.AtOnce(); n != 3 {
		t.Fatalf("the user's 3 over the plugin's 2 = %d", n)
	}
	plug.MaxAccounts = &none
	if n := plug.AtOnce(); n != 0 {
		t.Fatalf("the user's no limit over the plugin's 2 = %d", n)
	}
	if n := (Provider{ID: "keys", Key: "k"}).AtOnce(); n != 0 {
		t.Fatalf("no plugin, nothing set = %d", n)
	}

	p := Provider{ID: "at-once", Name: "At once", Chat: "https://example.invalid/v1", Key: "a", Keys: []KeyAccount{{Key: "b"}, {Key: "c"}}}
	if err := Save(p); err != nil {
		t.Fatal(err)
	}
	two := 2
	if err := SetAtOnce(p.ID, &two); err != nil {
		t.Fatal(err)
	}
	got, err := Find(p.ID)
	if err != nil {
		t.Fatal(err)
	}
	if got.AtOnce() != 2 || got.MaxAccounts == nil || *got.MaxAccounts != 2 {
		t.Fatalf("set to 2: %d %v", got.AtOnce(), got.MaxAccounts)
	}
	if err := SetAtOnce(p.ID, new(int)); err != nil {
		t.Fatal(err)
	}
	if got, _ = Find(p.ID); got.AtOnce() != 0 || got.MaxAccounts == nil {
		t.Fatalf("set to no limit: %d %v", got.AtOnce(), got.MaxAccounts)
	}
	if err := SetAtOnce(p.ID, nil); err != nil {
		t.Fatal(err)
	}
	if got, _ = Find(p.ID); got.MaxAccounts != nil {
		t.Fatalf("given back to the plugin's: %v", *got.MaxAccounts)
	}
	big := MaxLimit + 1
	if err := SetAtOnce(p.ID, &big); err == nil {
		t.Fatal("a limit past MaxLimit was taken")
	}
	neg := -1
	got.MaxAccounts = &neg
	if err := Save(*got); err != nil {
		t.Fatal(err)
	}
	if got, _ = Find(p.ID); got.MaxAccounts == nil || *got.MaxAccounts != 0 {
		t.Fatalf("a save of -1 kept %v, want 0", got.MaxAccounts)
	}
}
