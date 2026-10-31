package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// The Agents page's Install another agent says Plugins has more agents
// (the owner: 因为我们支持Agent插件了，这里是不是也应该显示插件中有更多？):
// /api/plugins/agents names the agents a suggested plugin adds, leaving
// out every other kind and one installed here, turned off or pinned to a
// version too.
func TestPluginAgentsNotInstalled(t *testing.T) {
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"version":1,"plugins":[
			{"package":"@magpie-community/opencode-x-auth","name":"X","community":true},
			{"package":"@magpie-community/middleware-guard","name":"Guard","kind":"middleware","community":true},
			{"package":"@magpie-community/agent-kilo","name":"Kilo Code","kind":"agent","community":true},
			{"package":"@magpie-community/agent-aider","name":"Aider","kind":"agent","community":true},
			{"package":"@magpie-community/agent-pig","name":"PiG","kind":"agent","community":true},
			{"package":"@magpie-community/agent-bub","name":"Bub","kind":"agent","community":true},
			{"package":"agent-elsewhere","name":"Elsewhere","kind":"agent"}
		]}`))
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_PLUGIN_MARKET", srv.URL)
	plugin.RefreshMarket()
	t.Cleanup(plugin.RefreshMarket)
	dir := settings.Dir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	installed := `{"plugins":[{"spec":"@magpie-community/agent-aider@0.1.3"},{"spec":"@magpie-community/agent-bub","off":true}]}`
	if err := os.WriteFile(filepath.Join(dir, "plugins.json"), []byte(installed), 0o644); err != nil {
		t.Fatal(err)
	}

	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest("GET", "/api/plugins/agents", nil))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	var got []string
	if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
		t.Fatal(err)
	}
	if want := []string{"Kilo Code", "PiG"}; !slices.Equal(got, want) {
		t.Errorf("got %q, want %q", got, want)
	}

	// every one installed: the page gets an empty list, never null
	if err := os.WriteFile(filepath.Join(dir, "plugins.json"), []byte(`{"plugins":[{"spec":"@magpie-community/agent-kilo"},{"spec":"@magpie-community/agent-aider"},{"spec":"@magpie-community/agent-pig"},{"spec":"@magpie-community/agent-bub"}]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	rec = httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest("GET", "/api/plugins/agents", nil))
	if body := rec.Body.String(); body != "[]\n" && body != "[]" {
		t.Errorf("all installed: %q, want []", body)
	}
}
