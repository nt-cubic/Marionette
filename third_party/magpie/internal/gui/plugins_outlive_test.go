package gui

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// An install goes on though the page's connection goes: on magpie web
// behind Cloudflare the tunnel gives up after 100 seconds and answers the
// page with its own 524 page (precisionz installing Zed, #2032), and bun
// stopped halfway leaves plugins/ half-written. So do a remove, an update
// and an upgrade.
func TestPluginOpsOutliveRequest(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("MAGPIE_PLUGIN_MARKET", "off")
	t.Setenv("PATH", t.TempDir())
	oldAdd, oldRemove, oldUpdate, oldUpgrade := addPlugin, removePlugin, updatePlugins, upgradePlugin
	t.Cleanup(func() {
		addPlugin, removePlugin, updatePlugins, upgradePlugin = oldAdd, oldRemove, oldUpdate, oldUpgrade
	})
	var got []string
	check := func(op string, ctx context.Context) {
		if _, deadline := ctx.Deadline(); ctx.Err() != nil || !deadline {
			got = append(got, op+": stopped with the page")
		} else {
			got = append(got, op+": going")
		}
	}
	addPlugin = func(ctx context.Context, spec string) (plugin.Entry, error) {
		check("add", ctx)
		return plugin.Entry{}, nil
	}
	removePlugin = func(ctx context.Context, spec string) error { check("remove", ctx); return nil }
	updatePlugins = func(ctx context.Context) error { check("update", ctx); return nil }
	upgradePlugin = func(ctx context.Context, name string) error { check("upgrade", ctx); return nil }
	mux := http.NewServeMux()
	pluginRoutes(mux, nil)
	for _, op := range []string{"add", "remove", "update", "upgrade"} {
		ctx, cancel := context.WithCancel(context.Background())
		cancel() // the tunnel gave up waiting
		r := httptest.NewRequest("POST", "/api/plugins/"+op, strings.NewReader(`{"spec":"@magpie-community/opencode-zed-auth"}`)).WithContext(ctx)
		mux.ServeHTTP(httptest.NewRecorder(), r)
	}
	if want := "add: going,remove: going,update: going,upgrade: going"; strings.Join(got, ",") != want {
		t.Fatalf("got %v, want %s", got, want)
	}
}
