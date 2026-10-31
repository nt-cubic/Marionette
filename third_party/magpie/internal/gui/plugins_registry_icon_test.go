package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// registry.json can give a listing a picture as its icon (Kilo Code's
// https://github.com/Kilo-Org.png) where most give a bundled icon's name.
// The page drew icon(<url>) as the generic cube. Both of Discover's
// answers now give such a picture kept as a file, as a package's own
// magpie.icon is, and leave a bundled name as it is. A data:image URI
// stands in for the URL here, so nothing is fetched; the https fetch is
// provider.TestMarketIconURLKept's.
func TestRegistryIconPictureKept(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("MAGPIE_BUN", "")
	t.Setenv("PATH", "")
	const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"version":1,"plugins":[
			{"package":"@magpie-community/agent-kilo","name":"Kilo Code","kind":"agent","community":true,"icon":"` + png + `"},
			{"package":"@magpie-community/opencode-copilot-auth","name":"GitHub Copilot","icon":"githubcopilot","community":true},
			{"package":"@magpie-community/tools-plain","name":"Plain","kind":"tools","icon":"javascript:alert(1)","community":true}
		]}`))
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_PLUGIN_MARKET", srv.URL)
	plugin.RefreshMarket()
	t.Cleanup(plugin.RefreshMarket)
	plugin.ReloadInfo()
	t.Cleanup(plugin.ReloadInfo)

	icons := func(ls []pluginListingJSON) map[string]string {
		out := map[string]string{}
		for _, l := range ls {
			out[l.Package] = l.Icon
		}
		return out
	}
	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest("GET", "/api/plugins/listings", nil))
	var ls struct{ Listings []pluginListingJSON }
	if err := json.Unmarshal(rec.Body.Bytes(), &ls); err != nil {
		t.Fatalf("%d %s: %v", rec.Code, rec.Body, err)
	}
	got := icons(ls.Listings)
	if ic := got["@magpie-community/agent-kilo"]; !strings.HasPrefix(ic, "file:") {
		t.Errorf("Kilo Code's picture = %.60q, want it kept as a file", ic)
	}
	if ic := got["@magpie-community/opencode-copilot-auth"]; ic != "githubcopilot" {
		t.Errorf("a bundled name = %q", ic)
	}
	if ic := got["@magpie-community/tools-plain"]; ic != "" {
		t.Errorf("a picture magpie doesn't take = %q", ic)
	}
	// the whole market at once (pluginMarketState), npm not asked here
	var asked []pluginListingJSON
	for _, l := range plugin.Market(t.Context()) {
		asked = append(asked, pluginListingJSON{Listing: l})
	}
	listingIcons(asked, 0)
	if ic := icons(asked)["@magpie-community/agent-kilo"]; !strings.HasPrefix(ic, "file:") {
		t.Errorf("market: Kilo Code's picture = %.60q", ic)
	}
	// what the market keeps is the registry's own word, not the page's
	for _, l := range plugin.Market(t.Context()) {
		if l.Package == "@magpie-community/agent-kilo" && l.Icon != png {
			t.Errorf("the market's listing was changed to %.60q", l.Icon)
		}
	}
}
