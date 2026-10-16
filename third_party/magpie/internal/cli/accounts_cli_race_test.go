package cli

import (
	"fmt"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/agentenv"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
)

// A provider no longer known to LoginUsage has no quota, but its remembered
// account still has to be listed. Fast empty results also exercise the same
// concurrent writes as cached usage, without a vendor or a real sign-in.
func TestAccountRowsConcurrentUsage(t *testing.T) {
	home := t.TempDir()
	for _, key := range agentenv.Vars {
		t.Setenv(key, "")
	}
	for key, dir := range map[string]string{
		"HOME": home, "USERPROFILE": home,
		"XDG_CONFIG_HOME": filepath.Join(home, ".config"),
		"XDG_CACHE_HOME":  filepath.Join(home, ".cache"),
		"XDG_DATA_HOME":   filepath.Join(home, ".local", "share"),
		"APPDATA":         filepath.Join(home, "AppData", "Roaming"),
		"LOCALAPPDATA":    filepath.Join(home, "AppData", "Local"),
	} {
		t.Setenv(key, dir)
	}
	plugin.UseCached([]plugin.Provider{})
	t.Cleanup(func() { plugin.UseCached(nil) })
	now := time.Date(2026, time.October, 3, 12, 0, 0, 0, time.UTC)

	for _, tc := range []struct {
		name      string
		accounts  int
		providers int
		reads     int
	}{
		{"single-provider", 1, 1, 1},
		{"multiple-providers", 256, 256, 8},
		{"same-provider-accounts", 256, 1, 8},
	} {
		t.Run(tc.name, func(t *testing.T) {
			logins := make([]provider.Login, tc.accounts)
			want := make([]accountRow, tc.accounts)
			for i := range logins {
				l := provider.Login{
					Agent: fmt.Sprintf("contribution-race-%03d", i%tc.providers),
					User:  fmt.Sprintf("account-%03d@example.test", i),
					Plan:  "Synthetic", Seen: now, On: true,
				}
				logins[i] = l
				want[i] = accountRow{Agent: l.Agent, User: l.User, Plan: l.Plan, On: l.On, Windows: []quotaSpan{}}
			}
			for i := range tc.reads {
				got := accountRows(logins, now)
				if !reflect.DeepEqual(got, want) {
					t.Fatalf("read %d: account rows = %+v, want %+v", i, got, want)
				}
			}
			t.Logf("listed %d accounts across %d providers with empty usage on %d reads", tc.accounts, tc.providers, tc.reads)
		})
	}
}
