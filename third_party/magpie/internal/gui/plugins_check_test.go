package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

// Check for updates is a route of its own, not one of the plugin ops: it
// answers with what npm said of each plugin (none here, so npm is never
// asked) and the list as it then stands.
func TestPluginsCheckRoute(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest("POST", "/api/plugins/check", nil))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	var got struct {
		At      string
		Plugins []any
		State   *pluginsJSON
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if got.At == "" || got.Plugins == nil || got.State == nil || got.State.Plugins == nil {
		t.Fatalf("answered %s", rec.Body)
	}
}
