package agent

import (
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/edit"
)

// The route the Codex app's app-server is restarted on (codex_app_server.go)
// changes when Codex is connected beside its ChatGPT sign-in and when it is
// disconnected (JiangLiuer222 on X: the app kept sending to 127.0.0.1:3425
// after Disconnect), and not when the Codex app writes a model pick of its
// own, which would restart it on every pick.
func TestCodexAppRouteFollowsConnectAndDisconnect(t *testing.T) {
	home, read := codexHome(t, `{"tokens":{"access_token":"x","id_token":"x.e30.x"}}`, "model = \"gpt-5.5\"\n")
	cx := codex(home)
	route := func() string {
		t.Helper()
		r, _, known := codexAppRoute()
		if !known {
			t.Fatal("route not known")
		}
		return r
	}
	before := route()
	if _, err := cx.ConnectHow(); err != nil {
		t.Fatal(err)
	}
	connected := route()
	if connected == before {
		t.Fatalf("connect left the route as it was:\n%s", read())
	}
	// the Codex app writes its own pick: no restart for that
	if err := edit.SetTOMLTop(filepath.Join(home, ".codex", "config.toml"), edit.KV{Path: "model", Value: "gpt-6.1-sol"}); err != nil {
		t.Fatal(err)
	}
	if route() != connected {
		t.Fatalf("a model pick changed the route:\n%s", read())
	}
	if err := cx.Disconnect(); err != nil {
		t.Fatal(err)
	}
	// the [model_providers.magpie] table stays, for the threads started
	// on it; openai_base_url goes
	if r := route(); r == connected {
		t.Fatalf("disconnect left the route as it was: %q\n%s", r, read())
	}
}
