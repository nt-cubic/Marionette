package usageapi

import (
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
