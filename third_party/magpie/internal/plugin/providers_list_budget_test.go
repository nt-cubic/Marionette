package plugin

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The budgets a listing is given must add up: each account's models hook
// gets listWait, so a provider with two accounts listed back to back costs
// two, and a renewal waited for before the hook spends the hook's budget
// besides. One provider could then be allowed 35s (RENEW_WAIT 15s +
// listWait 20s) — over the 30s refreshDeadline and the editor's own
// budget — and one slow sign-in took every provider's list down with it
// (#4065).

// A provider whose models hook is slow for each of its two accounts, with
// no renewal due (no auth.refresh): listing it side by side costs one
// listWait, back to back costs two.
func listBudgetPlugin(t *testing.T, hookMillis int) string {
	t.Helper()
	spec := filepath.Join(t.TempDir(), "plugin.mjs")
	js := fmt.Sprintf(`export default async () => ({
  config(c) { c.provider ??= {}; c.provider.budget = { name: "BudgetCo", models: { "b-1": { name: "B One" } } } },
  auth: { provider: "budget", methods: [{ type: "api", label: "API key" }] },
  provider: { id: "budget", models: async p => { await new Promise(r => setTimeout(r, %d)); return p.models } }
})`, hookMillis)
	if err := os.WriteFile(spec, []byte(js), 0o600); err != nil {
		t.Fatal(err)
	}
	return spec
}

// A two-account provider is listed with its accounts side by side: one
// provider costs one listWait, not one per account, and stays under the
// caller's budget.
func TestProviderListCoversAccountsSideBySide(t *testing.T) {
	sandbox(t)
	// each hook is given listWait and finishes within it; two of them in a
	// row do not fit the caller's budget, side by side they do
	oldList := pluginListWait
	pluginListWait = 4 * time.Second
	t.Cleanup(func() { pluginListWait = oldList })
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	abs, _ := filepath.Abs(listBudgetPlugin(t, 3000))
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	for _, who := range []string{"one", "two"} {
		if _, err := Import(ctx, "budget", map[string]any{"type": "oauth", "access": "a-" + who, "accountId": who + "@budget"}); err != nil {
			t.Fatal(err)
		}
	}
	// the editor's own budget: a listing that fits the host's own bounds
	// is not killed by the caller's
	call, stop := context.WithTimeout(context.Background(), 4500*time.Millisecond)
	defer stop()
	ps, err := Providers(call)
	if err != nil {
		t.Fatalf("a two-account provider listed back to back outlived the caller's budget: %v", err)
	}
	if len(ps) != 1 || ps[0].ID != "budget" || len(ps[0].Models) == 0 {
		t.Fatalf("Providers = %+v, want budget's models", ps)
	}
	if len(ps[0].Accounts) != 2 {
		t.Fatalf("Accounts = %+v, want both listed", ps[0].Accounts)
	}
}

// A sign-in due for renewal is waited for before the models hook runs, and
// that wait spends the hook's own budget: one provider then costs one
// listWait in a listing, not listWait plus the renewal wait, so it stays
// under refreshDeadline and one renewing sign-in no longer takes the
// whole listing down with it (#4065).
func TestRenewalWaitSpendsTheListBudget(t *testing.T) {
	sandbox(t)
	// the renewal never finishes (a vendor stuck); the wait for it is
	// bounded by the host's own RENEW_WAIT (15s). The hook takes 10s and is
	// given listWait = 11s of its own, so back to back the provider costs
	// 25s — over the 20s refreshDeadline. With the wait spent out of the
	// hook's budget the listing ends at ~15s: the provider falls back, the
	// list stands.
	spec := filepath.Join(t.TempDir(), "plugin.mjs")
	js := `export default async () => ({
  config(c) { c.provider ??= {}; c.provider.renewbudget = { name: "RenewBudget", models: { "rb-1": { name: "RB One" } } } },
  auth: {
    provider: "renewbudget",
    methods: [{ type: "api", label: "API key" }],
    async refresh(auth) { await new Promise(ok => setTimeout(ok, 60000)); return { access: "a-1", refresh: auth.refresh + "+", expires: Date.now() + 3600e3 } }
  },
  provider: { id: "renewbudget", models: async p => { await new Promise(r => setTimeout(r, 10000)); return p.models } }
})`
	if err := os.WriteFile(spec, []byte(js), 0o600); err != nil {
		t.Fatal(err)
	}
	oldList, oldDeadline := pluginListWait, refreshDeadline
	pluginListWait, refreshDeadline = 11*time.Second, 20*time.Second
	t.Cleanup(func() { pluginListWait, refreshDeadline = oldList, oldDeadline })
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	abs, _ := filepath.Abs(spec)
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	soon := time.Now().Add(time.Minute).UnixMilli() // inside the 5-minute lead
	if _, err := Import(ctx, "renewbudget", map[string]any{"type": "oauth", "refresh": "r", "access": "old", "expires": soon, "accountId": "due@renewbudget"}); err != nil {
		t.Fatal(err)
	}
	UseCached(nil)

	Cached()
	Refreshed()
	if !refreshCacheGood() {
		t.Fatal("a renewal waited for before its provider's models hook took the whole listing past refreshDeadline")
	}
	ps, ok := Listed()
	if !ok || len(ps) != 1 || ps[0].ID != "renewbudget" {
		t.Fatalf("Listed = %+v, %v", ps, ok)
	}
	// the hook out of its own time is that provider's fallback, said so,
	// not a bare deadline for every provider
	if !ps[0].FellBack || !strings.Contains(ps[0].ListError, "didn't finish") {
		t.Fatalf("FellBack = %v, ListError = %q, want the hook given up on alone, said so", ps[0].FellBack, ps[0].ListError)
	}
}
