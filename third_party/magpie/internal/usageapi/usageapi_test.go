package usageapi

import (
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/usage"
)

func TestMain(m *testing.M) { testenv.Main(m) }

// A provider the usage log names but that isn't set up now is still named
// as magpie knows it (@ByronVon1 on X): the Claude Code subscription,
// signed out of since, showed as "claude" in the Requests page's Provider
// filter, between providers named by their names. A preset's removed
// since is named by the preset; a custom one magpie can't know stays its
// id. Every place the page names a provider from the log says the same:
// the filter, each row, the split-by legends, and the summary's models
// and accounts.
func TestProvidersNotSetUpAreNamed(t *testing.T) {
	for _, p := range provider.All() {
		if p.ID == "claude" || p.ID == "deepseek" {
			t.Fatalf("the test home has %s set up", p.ID)
		}
	}
	now := time.Now()
	for i, pid := range []string{"claude", "deepseek", "gone-custom"} {
		usage.Append(usage.Record{Time: now.Add(-time.Duration(i+1) * time.Minute), Agent: "claude", Provider: pid, Model: "m-" + pid, Input: 10, Output: 2, ProviderAccount: "me@example.com"})
	}
	want := map[string]string{"claude": "Claude Code", "deepseek": "DeepSeek", "gone-custom": "gone-custom"}

	l := LedgerPage(usage.All, usage.Filter{}, 0, 100)
	if len(l.Providers) != len(want) {
		t.Fatalf("providers: %+v", l.Providers)
	}
	for _, p := range l.Providers {
		if p.Name != want[p.ID] {
			t.Errorf("the Provider filter names %s %q, want %q", p.ID, p.Name, want[p.ID])
		}
	}
	for _, r := range l.Rows {
		if r.ProviderName != want[r.Provider] {
			t.Errorf("a row names %s %q, want %q", r.Provider, r.ProviderName, want[r.Provider])
		}
	}
	for _, s := range l.By["provider"] {
		if s.Name != want[s.ID] {
			t.Errorf("the split by provider names %s %q, want %q", s.ID, s.Name, want[s.ID])
		}
	}
	for _, s := range l.By["modelAt"] {
		if s.Name == "m-claude · claude" {
			t.Errorf("the split by model at provider names %q", s.Name)
		}
	}
	if len(l.Accounts) != 1 || len(l.Accounts[0].Providers) != 3 {
		t.Errorf("accounts: %+v", l.Accounts)
	}
	for _, a := range l.Accounts {
		for _, n := range a.Providers {
			if n == "claude" || n == "deepseek" {
				t.Errorf("the Account filter names a provider %q", n)
			}
		}
	}

	s := State(usage.All)
	for _, g := range append(s.Models, s.Accounts...) {
		if g.Sub == "claude" || g.Sub == "deepseek" {
			t.Errorf("the summary names %q's provider %q", g.Name, g.Sub)
		}
	}
	if len(s.Models) != 3 {
		t.Errorf("models: %+v", s.Models)
	}
}

// cybercaius on Discord: the Usage page's Provider filter lists a
// provider deleted and the one added under its id since apart, the
// deleted one by its own name with when it was deleted.
func TestDeletedProviderListedApartAndMarked(t *testing.T) {
	home := t.TempDir()
	testenv.SetHome(t, home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Old relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-9"}}); err != nil {
		t.Fatal(err)
	}
	usage.Append(usage.Record{Time: time.Now().Add(-time.Hour), Agent: "codex", Provider: "relay", Model: "gpt-9", Input: 10, Output: 2})
	if err := provider.Delete("relay"); err != nil {
		t.Fatal(err)
	}
	// while no other has its id, its calls stay under it, named as it was
	if ps := LedgerPage(usage.All, usage.Filter{}, 0, 100).Providers; len(ps) != 1 || ps[0].ID != "relay" || ps[0].Name != "Old relay" || ps[0].Deleted == nil {
		t.Fatalf("deleted, its id not taken: %+v", ps)
	}
	if err := provider.Save(provider.Provider{ID: "relay", Name: "New relay", Key: "k2", Chat: "http://127.0.0.1:2/v1", Models: []string{"gpt-9"}}); err != nil {
		t.Fatal(err)
	}
	usage.Append(usage.Record{Time: time.Now().Add(time.Second), Agent: "codex", Provider: "relay", Model: "gpt-9", Input: 10, Output: 2})

	got := map[string]Named{}
	for _, p := range LedgerPage(usage.All, usage.Filter{}, 0, 100).Providers {
		got[p.Name] = p
	}
	if n := got["New relay"]; n.ID != "relay" || n.Deleted != nil {
		t.Errorf("the new one: %+v", n)
	}
	if n := got["Old relay"]; !strings.HasPrefix(n.ID, "relay~") || n.Deleted == nil {
		t.Errorf("the deleted one: %+v (all %+v)", n, got)
	}
}

// CyberCaius on Discord: a deleted provider can be shown by a name of the
// user's, and once merged into another it is listed as that one, which
// names it among its merged ones so the page can part them again.
func TestDeletedProviderNamedAndMerged(t *testing.T) {
	home := t.TempDir()
	testenv.SetHome(t, home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	for _, p := range []provider.Provider{
		{ID: "relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-9"}},
		{ID: "main", Name: "Main", Key: "k2", Chat: "http://127.0.0.1:2/v1", Models: []string{"gpt-9"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	usage.Append(usage.Record{Time: time.Now().Add(-time.Hour), Agent: "codex", Provider: "relay", Model: "gpt-9", Input: 10, Output: 2})
	usage.Append(usage.Record{Time: time.Now().Add(-time.Minute), Agent: "codex", Provider: "main", Model: "gpt-9", Input: 10, Output: 2})
	if err := provider.Delete("relay"); err != nil {
		t.Fatal(err)
	}
	faces := func() map[string]Named {
		got := map[string]Named{}
		for _, p := range LedgerPage(usage.All, usage.Filter{}, 0, 100).Providers {
			got[p.ID] = p
		}
		return got
	}
	if err := provider.NameDeleted("relay", "  Leee (old key)  "); err != nil {
		t.Fatal(err)
	}
	if n := faces()["relay"]; n.Name != "Leee (old key)" || n.Deleted == nil {
		t.Fatalf("named: %+v", n)
	}
	if err := provider.NameDeleted("relay", ""); err != nil {
		t.Fatal(err)
	}
	if n := faces()["relay"]; n.Name != "relay" {
		t.Fatalf("its own name again: %+v", n)
	}
	if err := provider.NameDeleted("relay", "Leee"); err != nil {
		t.Fatal(err)
	}
	if err := provider.MergeDeleted("relay", "main"); err != nil {
		t.Fatal(err)
	}
	got := faces()
	if _, ok := got["relay"]; ok || len(got) != 1 {
		t.Fatalf("merged, still listed: %+v", got)
	}
	if m := got["main"].Merged; len(m) != 1 || m[0].ID != "relay" || m[0].Name != "Leee" {
		t.Fatalf("main's merged: %+v", got["main"])
	}
	if err := provider.MergeDeleted("relay", ""); err != nil {
		t.Fatal(err)
	}
	if got := faces(); got["relay"].Name != "Leee" || got["main"].Merged != nil {
		t.Fatalf("parted: %+v", got)
	}
	if err := provider.NameDeleted("main", "x"); err == nil {
		t.Fatal("named a provider that is not deleted")
	}
}
