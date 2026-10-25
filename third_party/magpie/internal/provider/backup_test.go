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

// Restoring and syncing a keyless provider keep this machine's balance
// token. A token supplied explicitly, or keys supplied with none, replaces it.
// A Volcengine access key (#1427) is kept the same way, the ID and Secret
// together.
func TestBackupBalanceToken(t *testing.T) {
	for _, op := range []struct {
		name string
		put  func([]Provider) error
	}{
		{"restore", func(ps []Provider) error { _, _, err := Restore(ps); return err }},
		{"mirror", func(ps []Provider) error { return Mirror(ps, nil) }},
	} {
		t.Run(op.name, func(t *testing.T) {
			for _, c := range []struct {
				name     string
				incoming Provider
				want     string
			}{
				{"without keys", Provider{}, "balance-here"},
				{"explicit token without keys", Provider{BalanceToken: "balance-new"}, "balance-new"},
				{"primary key with token", Provider{Key: "sk-new", BalanceToken: "balance-new"}, "balance-new"},
				{"primary key without token", Provider{Key: "sk-new"}, ""},
				{"spare key with token", Provider{Keys: []KeyAccount{{Key: "sk-new"}}, BalanceToken: "balance-new"}, "balance-new"},
				{"spare key without token", Provider{Keys: []KeyAccount{{Key: "sk-new"}}}, ""},
			} {
				t.Run(c.name, func(t *testing.T) {
					isolate(t)
					h := t.TempDir()
					t.Setenv("HOME", h)
					t.Setenv("USERPROFILE", h)
					t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
					t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
					t.Setenv("PATH", h)
					here := Provider{ID: "relay", Name: "Relay old", Chat: "https://old.example.com/v1",
						Key: "sk-here", KeyName: "main", KeyProtocol: Chat,
						Keys: []KeyAccount{{Name: "spare", Key: "sk-spare", Protocol: Anthropic}}, BalanceToken: "balance-here",
						AccessKeyID: "AK-here", SecretAccessKey: "SK-here"}
					if err := Save(here); err != nil {
						t.Fatal(err)
					}
					p := c.incoming
					p.ID, p.Name, p.Chat = "relay", "Relay new", "https://new.example.com/v1"
					if err := op.put([]Provider{p}); err != nil {
						t.Fatal(err)
					}
					ps, _ := Stored()
					if len(ps) != 1 {
						t.Fatalf("providers: %+v", ps)
					}
					got := ps[0]
					if got.BalanceToken != c.want || got.Name != p.Name || got.Chat != p.Chat {
						t.Fatalf("restored: %+v, want balance token %q and the incoming name and URL", got, c.want)
					}
					wantKeys := p
					if p.Key == "" && len(p.Keys) == 0 {
						wantKeys = here
					}
					if got.AccessKeyID != wantKeys.AccessKeyID || got.SecretAccessKey != wantKeys.SecretAccessKey {
						t.Fatalf("access key: %q %q, want %q %q", got.AccessKeyID, got.SecretAccessKey, wantKeys.AccessKeyID, wantKeys.SecretAccessKey)
					}
					if got.Key != wantKeys.Key || got.KeyName != wantKeys.KeyName || got.KeyProtocol != wantKeys.KeyProtocol || !slices.Equal(got.Keys, wantKeys.Keys) {
						t.Fatalf("keys: %+v, want %+v", got, wantKeys)
					}
				})
			}
		})
	}
}

// A backup or a sync carries the plugins added from npm or git, and the
// computer it comes to installs the ones it hasn't: the providers they
// serve come with them (yapechan on Discord: after a WebDAV sync, no
// providers showed until each was added again by hand). A plugin from a
// folder stays on its own computer, one already here is left as the user
// has it, and a key in an option goes only with the keys.
func TestPluginsCarried(t *testing.T) {
	isolate(t)
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("PATH", h)
	list := filepath.Join(filepath.Dir(plugin.Dir()), "plugins.json")
	write := func(body string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Dir(list), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(list, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write(`{"plugins":[
		{"spec":"@magpie-community/opencode-cursor-auth@0.3.0"},
		{"spec":"/Users/me/my-plugin"},
		{"spec":"@magpie-community/middleware-model-map","off":true,
		 "options":{"map":{"a":"b"},"apiKey":"sk-option","nested":{"authToken":"t","keep":1}}}]}`)

	keyless := Carried(false)
	if len(keyless) != 2 || keyless[0].Spec != "@magpie-community/opencode-cursor-auth@0.3.0" {
		t.Fatalf("carried: %+v, want the npm plugins only", keyless)
	}
	opts := keyless[1].Options
	if !keyless[1].Off || opts["apiKey"] != nil || opts["nested"].(map[string]any)["authToken"] != nil ||
		opts["map"] == nil || opts["nested"].(map[string]any)["keep"] == nil {
		t.Fatalf("carried without keys: %+v, want it off, its options with no key in them", keyless[1])
	}
	withKeys := Carried(true)
	if withKeys[1].Options["apiKey"] != "sk-option" {
		t.Fatalf("carried with keys: %+v, want the option's key too", withKeys[1])
	}

	// the other computer: the model-map middleware is here already, as the
	// user set it; the cursor plugin is not
	write(`{"plugins":[{"spec":"@magpie-community/middleware-model-map@1.0.0","options":{"map":{"x":"y"}}}]}`)
	oldAdd, oldHand := addPluginExact, handOver
	t.Cleanup(func() { addPluginExact, handOver = oldAdd, oldHand })
	var asked []string
	// the spec as the other computer has it, never resolved as a name
	addPluginExact = func(_ context.Context, spec string) (plugin.Entry, error) {
		asked = append(asked, spec)
		l := plugin.Load()
		l.Plugins = append(l.Plugins, plugin.Entry{Spec: spec})
		b, _ := json.Marshal(l)
		write(string(b))
		return plugin.Entry{Spec: spec}, nil
	}
	handed := 0
	handOver = func(context.Context, bool) map[string]error { handed++; return nil }
	added, failed := AddPlugins(context.Background(), withKeys)
	if len(failed) > 0 || !slices.Equal(added, []string{"@magpie-community/opencode-cursor-auth@0.3.0"}) ||
		!slices.Equal(asked, added) {
		t.Fatalf("added %v (asked %v, failed %v), want only the cursor plugin", added, asked, failed)
	}
	if handed != 1 {
		t.Fatalf("handed over %d times, want once: the plugin serves a built-in", handed)
	}
	got := plugin.Load().Plugins
	if len(got) != 2 || got[0].Options["map"].(map[string]any)["x"] != "y" {
		t.Fatalf("plugins here: %+v, want the middleware as the user set it", got)
	}

	// one turned off there comes off, with its options
	write(`{"plugins":[]}`)
	asked = nil
	if _, failed := AddPlugins(context.Background(), keyless[1:]); len(failed) > 0 {
		t.Fatal(failed)
	}
	if got := plugin.Load().Plugins; len(got) != 1 || !got[0].Off || got[0].Options["map"] == nil {
		t.Fatalf("plugins here: %+v, want the middleware off with its options", got)
	}

	// nothing to add: nothing installed, nothing handed over
	asked, handed = nil, 0
	AddPlugins(context.Background(), keyless[1:])
	if len(asked) != 0 || handed != 0 {
		t.Fatalf("asked %v, handed over %d times, for plugins already here", asked, handed)
	}
}

// The subscriptions a backup or a sync brings the settings of, with no
// account here, are named: they show once signed in, and nothing else says
// so (yapechan on Discord).
func TestNotSignedIn(t *testing.T) {
	isolate(t)
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("PATH", h)
	t.Setenv("CLAUDE_CONFIG_DIR", filepath.Join(h, ".claude"))
	t.Setenv("CODEX_HOME", filepath.Join(h, ".codex"))
	ps := []Provider{
		{ID: "claude", Models: []string{"claude-opus-4-8"}},
		{ID: "cursor", Models: []string{"auto"}},
		{ID: "codex", Hidden: true},
		{ID: "relay", Name: "Relay", Chat: "https://relay.example.invalid/v1", Key: "sk"},
		{ID: "claude", Models: []string{"claude-opus-4-8"}},
	}
	if got := NotSignedIn(ps); !slices.Equal(got, []string{"claude", "cursor"}) {
		t.Fatalf("not signed in: %v, want claude and cursor", got)
	}
}
