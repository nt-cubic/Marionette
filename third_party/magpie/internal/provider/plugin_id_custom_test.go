package provider

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// #867: a provider of the user's own saved before a plugin with its id was
// installed (zenmux, then @zenmux/pi-zenmux-oauth) hid the plugin's
// subscription once signed in — "signed in, but magpie can't list it". The
// plugin's is id-plugin then, and the user's own keeps its id.
func TestPluginIDBesideCustomProvider(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	if err := Save(Provider{ID: "fakeco", Name: "fakeco", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"mine-1", "mine-2"}}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if got := PluginID("fakeco"); got != "fakeco-plugin" {
		t.Fatalf("PluginID(fakeco) = %s", got)
	}
	id, err := PluginAPIKey(ctx, "fakeco", 0, nil, "k-123456")
	if err != nil {
		t.Fatal(err)
	}
	if id != "fakeco-plugin" {
		t.Errorf("signed in as %s", id)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if p, err := Find("fakeco-plugin"); err != nil || !p.IsPlugin() {
		t.Fatalf("Find(fakeco-plugin) = %+v, %v", p, err)
	}
	if p, err := Find("fakeco"); err != nil || p.IsPlugin() || len(p.Models) != 2 {
		t.Fatalf("Find(fakeco) = %+v, %v", p, err)
	}
	// and the user's own goes when removed, the plugin's staying
	if err := Delete("fakeco"); err != nil {
		t.Fatal(err)
	}
	if _, err := Find("fakeco"); err == nil {
		if p, _ := Find("fakeco"); !p.IsPlugin() {
			t.Errorf("fakeco is still the user's own after rm")
		}
	}
}
