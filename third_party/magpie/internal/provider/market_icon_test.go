package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
)

// registry.json gives Kilo Code, Search1API and PowerContext an https
// picture as their icon, not a bundled icon's name. A provider or agent
// the market gives such an icon shows it as a plugin's own picture is
// shown: fetched through FetchIcon's checks and kept, "" until it is in,
// and never the URL itself, which the page took for a bundled file's
// name and drew as the generic cube. A bundled name is given as it is,
// and a picture magpie doesn't take is refused.
func TestMarketIconURLKept(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"version":1,"plugins":[
			{"package":"@magpie-community/agent-kilo","name":"Kilo Code","kind":"agent","community":true,"icon":"https://kilo.example/kilo.png","providers":["kilo-p"]},
			{"package":"@magpie-community/opencode-copilot-auth","name":"GitHub Copilot","icon":"githubcopilot","providers":["github-copilot"],"community":true},
			{"package":"@magpie-community/opencode-plain-auth","name":"Plain","icon":"http://plain.example/x.png","providers":["plain"],"community":true}
		]}`))
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_PLUGIN_MARKET", srv.URL)
	plugin.RefreshMarket()
	t.Cleanup(plugin.RefreshMarket)
	plugin.Market(context.Background())

	png := []byte("\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR")
	var fetched atomic.Int32
	was := fetchPluginIcon
	t.Cleanup(func() { fetchPluginIcon = was })
	fetchPluginIcon = func(_ context.Context, u string) (string, error) {
		fetched.Add(1)
		if u != "https://kilo.example/kilo.png" {
			t.Errorf("fetched %q", u)
		}
		return StoreIcon(png)
	}

	kilo := plugin.Provider{ID: "kilo-p", Spec: "@magpie-community/agent-kilo"}
	got := PluginIcon(kilo)
	if strings.Contains(got, "://") {
		t.Fatalf("the market's https icon went to the page as %q", got)
	}
	for i := 0; i < 200 && !strings.HasPrefix(got, "file:"); i++ {
		time.Sleep(20 * time.Millisecond)
		got = PluginIcon(kilo)
	}
	if !strings.HasPrefix(got, "file:") || IconFile(strings.TrimPrefix(got, "file:")) == "" {
		t.Fatalf("after the fetch = %q, want a kept picture", got)
	}
	if ic := MarketIcon(plugin.Icon(kilo.Spec, kilo.ID)); ic != got {
		t.Errorf("MarketIcon = %q, PluginIcon = %q", ic, got)
	}
	if ic := PluginIcon(plugin.Provider{ID: "github-copilot", Spec: "@magpie-community/opencode-copilot-auth"}); ic != "githubcopilot" {
		t.Errorf("a bundled name = %q", ic)
	}
	if ic := PluginIcon(plugin.Provider{ID: "plain", Spec: "@magpie-community/opencode-plain-auth"}); ic != "" {
		t.Errorf("plain http = %q, want refused", ic)
	}
	if n := fetched.Load(); n != 1 {
		t.Errorf("fetched %d times", n)
	}
}
