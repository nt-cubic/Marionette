package provider

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// leftPlugins installs two plugins from folders (so removing one runs no
// Bun) and lists their providers as the host last told them: foo's
// plugin serves foo, zap and shared; bar's serves bar and shared.
func leftPlugins(t *testing.T) (foo, bar string) {
	t.Helper()
	dir := t.TempDir()
	foo, bar = filepath.Join(dir, "foo"), filepath.Join(dir, "bar")
	for spec, name := range map[string]string{foo: "@x/opencode-foo-auth", bar: "@x/opencode-bar-auth"} {
		if err := os.MkdirAll(spec, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(spec, "package.json"), []byte(`{"name":"`+name+`","version":"1.0.0"}`), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	list := filepath.Join(filepath.Dir(plugin.Dir()), "plugins.json")
	if err := os.MkdirAll(filepath.Dir(list), 0o700); err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(map[string]any{"plugins": []map[string]string{{"spec": foo}, {"spec": bar}}})
	if err := os.WriteFile(list, b, 0o600); err != nil {
		t.Fatal(err)
	}
	plugin.UseCached([]plugin.Provider{
		{ID: "foo", Spec: foo}, {ID: "zap", Spec: foo}, {ID: "shared", Spec: foo},
		{ID: "bar", Spec: bar}, {ID: "shared", Spec: bar},
	})
	t.Cleanup(func() { plugin.UseCached(nil) })
	return foo, bar
}

// writeProviders puts ps in providers.json as the user's file has them.
func writeProviders(t *testing.T, ps []Provider) {
	t.Helper()
	if err := store(file{Providers: ps}); err != nil {
		t.Fatal(err)
	}
}

func storedIDs(t *testing.T) []string {
	t.Helper()
	ps, err := Stored()
	if err != nil {
		t.Fatal(err)
	}
	var ids []string
	for _, p := range ps {
		ids = append(ids, p.ID)
	}
	slices.Sort(ids)
	return ids
}

// Removing a plugin takes its providers' model picks out of providers.json
// with it, where nothing listed them and a sync asked for a sign-in to
// them on every run (#2057); every record the user made by hand, and every
// one another plugin or a built-in serves, stays as it was.
func TestRemovePluginSetsItsPicksAside(t *testing.T) {
	claudeHome(t)
	foo, _ := leftPlugins(t)
	writeProviders(t, []Provider{
		// the user's own provider named zap: the plugin's zap is
		// zap-plugin beside it
		{ID: "zap", Name: "My zap", Chat: "https://zap.example/v1", Key: "sk-mine", Models: []string{"z-1"}},
		// the plugin's: model picks alone
		{ID: "foo", Models: []string{"foo-1", "foo-2"}},
		{ID: "zap-plugin", Models: []string{"zap-big"}},
		// served by the other plugin too
		{ID: "shared", Models: []string{"s-1"}},
		// the other plugin's alone
		{ID: "bar", Models: []string{"bar-1"}},
		// a built-in's picks
		{ID: "claude", Models: []string{"claude-sonnet-4-5"}},
		// the user's own provider, an id no plugin has
		{ID: "mine", Chat: "https://mine.example/v1", Key: "sk-2"},
	})
	if got := NotSignedIn(mustStored(t)); !slices.Contains(got, "foo") {
		t.Fatalf("before: foo isn't among those asked to sign in: %v", got)
	}

	if err := RemovePlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	if got, want := storedIDs(t), []string{"bar", "claude", "mine", "shared", "zap"}; !slices.Equal(got, want) {
		t.Fatalf("providers.json after foo's plugin went: %v, want %v", got, want)
	}
	ps := mustStored(t)
	for _, p := range ps {
		if p.ID == "zap" && (p.Chat != "https://zap.example/v1" || p.Key != "sk-mine" || p.Name != "My zap") {
			t.Fatalf("the user's own zap was changed: %+v", p)
		}
	}
	if got := NotSignedIn(ps); slices.Contains(got, "foo") || slices.Contains(got, "zap-plugin") {
		t.Fatalf("a sync still asks for a sign-in to the removed plugin's providers: %v", got)
	}
	f, err := read()
	if err != nil {
		t.Fatal(err)
	}
	var left []string
	for _, l := range f.PluginLeft {
		if l.Package != "@x/opencode-foo-auth" {
			t.Fatalf("set aside under %q, not the plugin's package", l.Package)
		}
		left = append(left, l.Provider.ID)
	}
	slices.Sort(left)
	if !slices.Equal(left, []string{"foo", "zap-plugin"}) {
		t.Fatalf("set aside: %v", left)
	}

	// added again, its picks are as they were; the user's zap untouched
	oldAdd := addPlugin
	t.Cleanup(func() { addPlugin = oldAdd })
	addPlugin = func(_ context.Context, spec string) (plugin.Entry, error) { return plugin.Entry{Spec: spec}, nil }
	if _, err := AddPlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	if got, want := storedIDs(t), []string{"bar", "claude", "foo", "mine", "shared", "zap", "zap-plugin"}; !slices.Equal(got, want) {
		t.Fatalf("providers.json after foo's plugin came back: %v, want %v", got, want)
	}
	for _, p := range mustStored(t) {
		if p.ID == "foo" && !slices.Equal(p.Models, []string{"foo-1", "foo-2"}) {
			t.Fatalf("foo's picks came back as %v", p.Models)
		}
	}
	if f, _ := read(); len(f.PluginLeft) != 0 {
		t.Fatalf("still set aside: %+v", f.PluginLeft)
	}
}

// What the plugins serve not known (no listing kept, plugin-providers.json
// missing), removing one leaves providers.json as it was: unknown is never
// "none of them is the user's".
func TestRemovePluginUnknownListingKeepsRecords(t *testing.T) {
	claudeHome(t)
	foo, _ := leftPlugins(t)
	plugin.UseCached(nil)
	writeProviders(t, []Provider{{ID: "foo", Models: []string{"foo-1"}}, {ID: "bar", Models: []string{"bar-1"}}})
	if err := RemovePlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	if got := storedIDs(t); !slices.Equal(got, []string{"bar", "foo"}) {
		t.Fatalf("records went with no listing known: %v", got)
	}
}

// A record set aside whose id the user has given to a provider since
// stays aside when the plugin is added again, and theirs stays theirs.
func TestAddPluginLeavesTheUsersRecord(t *testing.T) {
	claudeHome(t)
	foo, _ := leftPlugins(t)
	writeProviders(t, []Provider{{ID: "foo", Models: []string{"foo-1"}}})
	if err := RemovePlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	mine := Provider{ID: "foo", Name: "Mine", Chat: "https://foo.example/v1", Key: "sk-3"}
	f, _ := read()
	f.Providers = append(f.Providers, mine)
	if err := store(f); err != nil {
		t.Fatal(err)
	}
	oldAdd := addPlugin
	t.Cleanup(func() { addPlugin = oldAdd })
	addPlugin = func(_ context.Context, spec string) (plugin.Entry, error) { return plugin.Entry{Spec: spec}, nil }
	if _, err := AddPlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	ps := mustStored(t)
	if len(ps) != 1 || ps[0].Chat != mine.Chat || ps[0].Key != mine.Key {
		t.Fatalf("the user's foo: %+v", ps)
	}
}

func mustStored(t *testing.T) []Provider {
	t.Helper()
	ps, err := Stored()
	if err != nil {
		t.Fatal(err)
	}
	return ps
}

// A record with an endpoint or a key is the user's own provider whatever
// its id, even one a removed plugin's provider would be listed under.
func TestRemovePluginKeepsTheUsersEndpoints(t *testing.T) {
	claudeHome(t)
	foo, _ := leftPlugins(t)
	writeProviders(t, []Provider{
		{ID: "zap", Chat: "https://zap.example/v1", Key: "sk-1"},
		{ID: "zap-plugin", Anthropic: "https://zap2.example", Key: "sk-2"},
		{ID: "foo", Models: []string{"foo-1"}},
	})
	if err := RemovePlugin(context.Background(), foo); err != nil {
		t.Fatal(err)
	}
	if got := storedIDs(t); !slices.Equal(got, []string{"zap", "zap-plugin"}) {
		t.Fatalf("providers.json: %v", got)
	}
}
