package gui

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// Two installed plugins that sign in to one provider (neiko on Discord:
// a third-party plugin beside their own folder plugin): each row of
// /api/plugins carries the clash, naming the plugin that serves it and
// the provider as it is listed, and prefer hands it to the other one.
func TestPluginsClashRows(t *testing.T) {
	bun := testenv.Bun(t)
	dir := t.TempDir()
	testenv.SetHome(t, dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(dir, "cache"))
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	t.Cleanup(plugin.Settle)
	src, err := os.ReadFile("../plugin/testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}
	mine, theirs := filepath.Join(dir, "mine", "index.js"), filepath.Join(dir, "theirs", "index.js")
	for _, f := range []string{mine, theirs} {
		if err := os.MkdirAll(filepath.Dir(f), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(f, src, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	type row struct {
		Spec      string
		Providers []string
		Clashes   []struct {
			ID, By, Name string
			With         []string
		}
	}
	do := func(path string, body any) map[string]row {
		t.Helper()
		method, rd := "GET", &bytes.Buffer{}
		if body != nil {
			method = "POST"
			_ = json.NewEncoder(rd).Encode(body)
		}
		rec := httptest.NewRecorder()
		mux.ServeHTTP(rec, httptest.NewRequest(method, path, rd))
		if rec.Code != 200 {
			t.Fatalf("%s: %d %s", path, rec.Code, rec.Body)
		}
		var s struct{ Plugins []row }
		if err := json.Unmarshal(rec.Body.Bytes(), &s); err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		out := map[string]row{}
		for _, r := range s.Plugins {
			out[r.Spec] = r
		}
		return out
	}
	// the rows' provider names are the plugins' as last asked (Cached),
	// asked again in the background once the plugins change: the list as
	// it stands once that answer is in
	answered := func(path string, body any) map[string]row {
		t.Helper()
		do(path, body)
		plugin.Refreshed()
		return do("/api/plugins", nil)
	}
	do("/api/plugins/add", map[string]string{"spec": mine})
	rows := answered("/api/plugins/add", map[string]string{"spec": theirs})
	check := func(rows map[string]row, by string) {
		t.Helper()
		for spec, other := range map[string]string{mine: theirs, theirs: mine} {
			r := rows[spec]
			if len(r.Clashes) != 1 || r.Clashes[0].ID != "fakeco" || r.Clashes[0].By != by || r.Clashes[0].Name != "FakeCo" || len(r.Clashes[0].With) != 1 || r.Clashes[0].With[0] != other {
				t.Fatalf("%s's row = %+v, want fakeco served by %s", spec, r, by)
			}
			if want := spec == by; (len(r.Providers) == 1) != want {
				t.Fatalf("%s's row lists %v; serving fakeco: %v", spec, r.Providers, want)
			}
		}
	}
	check(rows, theirs)
	check(answered("/api/plugins/prefer", map[string]string{"spec": mine, "provider": "fakeco"}), mine)
	check(do("/api/plugins", nil), mine)
	// removing theirs leaves mine alone, and no clash to say
	rows = answered("/api/plugins/remove", map[string]string{"spec": theirs})
	if r, ok := rows[mine]; !ok || len(r.Clashes) != 0 || len(r.Providers) != 1 || len(rows) != 1 {
		t.Fatalf("after removing theirs, rows = %+v", rows)
	}
}
