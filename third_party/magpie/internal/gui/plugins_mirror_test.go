package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/source"
)

// The Plugins page's 「国内镜像」 switch is kept in settings, which is
// where source asks whether it is on, and /api/plugins says how it is.
func TestPluginsMirrorSwitch(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	do := func(method, path, body string) map[string]any {
		t.Helper()
		rec := httptest.NewRecorder()
		mux.ServeHTTP(rec, httptest.NewRequest(method, path, strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%s %s: %d %s", method, path, rec.Code, rec.Body)
		}
		var v map[string]any
		if err := json.Unmarshal(rec.Body.Bytes(), &v); err != nil {
			t.Fatal(err)
		}
		return v
	}
	if v := do("GET", "/api/plugins", ""); v["mirror"] != false {
		t.Fatalf("at first: mirror %v", v["mirror"])
	}
	for _, on := range []bool{true, false} {
		b, _ := json.Marshal(map[string]bool{"on": on})
		if v := do("POST", "/api/plugins/mirror", string(b)); v["mirror"] != on {
			t.Fatalf("turned %v: answered %v", on, v)
		}
		if settings.Load().ChinaMirror != on || source.China() != on {
			t.Fatalf("turned %v: settings say %v, source %v", on, settings.Load().ChinaMirror, source.China())
		}
		if v := do("GET", "/api/plugins", ""); v["mirror"] != on {
			t.Fatalf("turned %v: /api/plugins says %v", on, v["mirror"])
		}
	}
}
