package provider

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// zenFree is the opencode-zen-free plugin's provider listing ids.
func zenFree(fellBack bool, ids ...string) []plugin.Provider {
	p := plugin.Provider{ID: "opencode-zen-free", Spec: "@magpie-community/opencode-zen-free-auth", Name: "OpenCode Zen Free",
		NPM: "@ai-sdk/openai-compatible", FellBack: fellBack}
	for _, id := range ids {
		p.Models = append(p.Models, plugin.Model{ID: id, Name: id, NPM: "@ai-sdk/openai-compatible"})
	}
	return []plugin.Provider{p}
}

// A magpie left running asks the plugins for their lists again, by
// itself: a free model Zen dropped leaves the list and the user's picks,
// a new one is listed, a pick typed in by hand stays, and a list that
// couldn't be read changes nothing (Jeremy.Zhou on Discord).
func TestPluginsListedAgainWhileRunning(t *testing.T) {
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", "")
	write := func(name string, v any) {
		t.Helper()
		b, _ := json.Marshal(v)
		if err := os.MkdirAll(settings.Dir(), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(settings.Dir(), name), b, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("plugins.json", map[string]any{"plugins": []map[string]any{{"spec": "@magpie-community/opencode-zen-free-auth"}}})
	write("plugin-auth.json", map[string]any{"opencode-zen-free": map[string]any{"type": "api", "key": "public"}})
	plugin.UseCached(zenFree(false, "big-pickle", "glm-5-free"))
	t.Cleanup(func() { plugin.UseCached(nil) })
	if err := Save(Provider{ID: "opencode-zen-free", Models: []string{"big-pickle", "glm-5-free", "mine-typed"}}); err != nil {
		t.Fatal(err)
	}

	answers := make(chan []plugin.Provider)
	asked := make(chan struct{})
	oldList, oldFirst, oldEvery := relistPlugins, relistFirst, relistEvery
	// asked says the loop asks again, so all it did with the last answer
	// is done
	relistPlugins = func(ctx context.Context) ([]plugin.Provider, error) {
		select {
		case asked <- struct{}{}:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
		select {
		case ps := <-answers:
			if !ps[0].FellBack {
				plugin.UseCached(ps) // as plugin.Providers keeps it
			}
			return ps, nil
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	relistFirst, relistEvery = time.Millisecond, time.Millisecond
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	t.Cleanup(func() {
		cancel()
		<-done
		relistPlugins, relistFirst, relistEvery = oldList, oldFirst, oldEvery
	})
	go func() { defer close(done); KeepPluginsListed(ctx) }()
	select {
	case <-asked:
	case <-time.After(10 * time.Second):
		t.Fatal("the plugins weren't asked")
	}

	answer := func(ps []plugin.Provider) {
		t.Helper()
		for _, step := range []func() bool{
			func() bool { answers <- ps; return true },
			func() bool { <-asked; return true },
		} {
			ok := make(chan bool, 1)
			go func() { ok <- step() }()
			select {
			case <-ok:
			case <-time.After(10 * time.Second):
				t.Fatal("the plugins weren't asked again")
			}
		}
	}
	picks := func() []string {
		t.Helper()
		p, err := Find("opencode-zen-free")
		if err != nil {
			t.Fatal(err)
		}
		return p.Models
	}
	available := func() []string {
		t.Helper()
		p, err := Find("opencode-zen-free")
		if err != nil {
			t.Fatal(err)
		}
		var ids []string
		for _, m := range p.Available() {
			ids = append(ids, m.ID)
		}
		return ids
	}

	// the vendor's list couldn't be read: the plugin told the one before
	answer(zenFree(true, "big-pickle"))
	if got := picks(); !slices.Equal(got, []string{"big-pickle", "glm-5-free", "mine-typed"}) {
		t.Fatalf("a list that fell back dropped picks: %v", got)
	}

	answer(zenFree(false, "big-pickle", "late-free"))
	if got := picks(); !slices.Equal(got, []string{"big-pickle", "mine-typed"}) {
		t.Fatalf("picks after the list was read again: %v", got)
	}
	if got := available(); !slices.Contains(got, "late-free") || slices.Contains(got, "glm-5-free") {
		t.Fatalf("models after the list was read again: %v", got)
	}
}
