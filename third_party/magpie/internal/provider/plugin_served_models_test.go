package provider

import (
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// A plugin provider offers the models any account switched on serves,
// not the first account's alone: a model only a later account lists is
// in the provider's catalog, and an account switched off adds nothing
// (#2554, willywang; LoKi saw it again on 0.1.1320). Each account keeps
// its own list for routing.
func TestPluginProviderOffersEveryOnAccountsModels(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "")
	spec := "@example/opencode-acme-auth"
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		os.MkdirAll(settings.Dir(), 0o700)
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": spec}}})
	write("plugin-auth.json", map[string]map[string]any{
		"acme":   {"type": "api", "key": "sk-acme-0001"},
		"acme#2": {"type": "api", "key": "sk-acme-0002"},
		"acme#3": {"type": "api", "key": "sk-acme-0003"},
	})
	plugin.UseCached([]plugin.Provider{{ID: "acme", Spec: spec, Name: "Acme",
		Methods: []plugin.Method{{Type: "api", Label: "API key"}},
		Accounts: []plugin.Account{
			{Key: "acme", Type: "api", Models: []string{"a"}},
			{Key: "acme#2", Type: "api", Models: []string{"a", "b"}},
			{Key: "acme#3", Type: "api", Models: []string{"c"}},
		},
		Models: listedModels(t, `[{"id": "a", "name": "A"}, {"id": "b", "name": "B"}, {"id": "c", "name": "C"}]`),
	}})
	t.Cleanup(func() { plugin.UseCached(nil) })
	saveLogins(t,
		savedLogin{Agent: "plugin:acme", User: "one", Home: "acme", On: true},
		savedLogin{Agent: "plugin:acme", User: "two", Home: "acme#2", On: true},
		savedLogin{Agent: "plugin:acme", User: "three", Home: "acme#3"},
	)
	ids := func(p Provider) string {
		var out []string
		for _, m := range p.Available() {
			out = append(out, m.ID)
		}
		return strings.Join(out, " ")
	}
	ps := pluginAccounts()
	i := slices.IndexFunc(ps, func(p Provider) bool { return p.ID == "acme" })
	if i < 0 {
		t.Fatalf("no acme provider in %v", ps)
	}
	if got := ids(ps[i]); got != "a b" {
		t.Fatalf("the provider offers %q, want %q", got, "a b")
	}
	if !ps[i].Account.Lists("a") || ps[i].Account.Lists("b") {
		t.Fatal("the first account routes past its own list")
	}
	per := map[string]string{}
	for _, l := range pluginLogins(mustCached(t, "acme")) {
		per[l.acct.Key] = ids(pluginProvider(mustCached(t, "acme"), l))
	}
	if per["acme"] != "a" || per["acme#2"] != "a b" || per["acme#3"] != "c" {
		t.Fatalf("each account's own models: %v", per)
	}
}

func mustCached(t *testing.T, id string) plugin.Provider {
	t.Helper()
	pp, ok := PluginOf(id)
	if !ok {
		t.Fatalf("no plugin %s", id)
	}
	return pp
}
