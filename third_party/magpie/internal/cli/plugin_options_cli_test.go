package cli

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
)

// optionsHome is a sandbox HOME with the Cursor plugin added, holding a
// middleware-style option in plugins.json, and the provider it declares
// a wire_trace switch for (as auth.options does); opts are its
// plugins.json options.
func optionsHome(t *testing.T, opts string) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("MAGPIE_BUN", "")
	dir := filepath.Join(home, ".config", "magpie")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	spec := "@magpie-community/opencode-cursor-auth"
	if err := os.WriteFile(filepath.Join(dir, "plugins.json"),
		[]byte(`{"plugins":[{"spec":"`+spec+`","options":`+opts+`}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	plugin.UseCached([]plugin.Provider{{ID: "cursor", Spec: spec, Name: "Cursor",
		Options: []plugin.Option{{Key: "wire_trace", Label: "Wire trace"}, {Key: "native", Label: "Native", Default: true}}}})
	t.Cleanup(func() { plugin.UseCached(nil) })
}

func entryOptions(t *testing.T) map[string]any {
	t.Helper()
	for _, e := range plugin.Load().Plugins {
		return e.Options
	}
	t.Fatal("no plugin")
	return nil
}

// plugins#550: `magpie plugin options cursor '{"wire_trace": true}'` wrote
// plugins.json, which the Cursor provider never reads; its switches are
// settings' PluginOptions, as the editor's Plugin options saves them.
func TestPluginOptionsSetsDeclaredSwitch(t *testing.T) {
	optionsHome(t, `{"keep":"me"}`)
	ctx := context.Background()
	out, err := stdoutOf(t, func() error { return pluginOptions(ctx, "cursor", []string{`{"wire_trace": true}`}) })
	if err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().PluginOptions["cursor"]; !got["wire_trace"] || len(got) != 1 {
		t.Fatalf("PluginOptions = %v; said %q", settings.Load().PluginOptions, out)
	}
	if o := entryOptions(t); len(o) != 1 || o["keep"] != "me" {
		t.Fatalf("plugins.json options = %v, want the middleware's left alone", o)
	}
	if !strings.Contains(out, "wire_trace is on for cursor") {
		t.Fatalf("said %q", out)
	}

	// shown with its value, beside what plugins.json holds
	out, err = stdoutOf(t, func() error { return pluginOptions(ctx, "cursor", nil) })
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "wire_trace  cursor on") || !strings.Contains(out, "native  cursor on") || !strings.Contains(out, `"keep"`) {
		t.Fatalf("show said %q", out)
	}

	// a mix: the switch to settings, the rest to plugins.json as before
	if _, err := stdoutOf(t, func() error {
		return pluginOptions(ctx, "cursor", []string{`{"native": false, "other": 1}`})
	}); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().PluginOptions["cursor"]; got["native"] || !got["wire_trace"] {
		t.Fatalf("PluginOptions = %v", got)
	}
	if o := entryOptions(t); len(o) != 1 || o["other"] != float64(1) {
		t.Fatalf("plugins.json options = %v, want only other", o)
	}

	// a switch takes true or false only, and nothing is written
	if _, err := stdoutOf(t, func() error {
		return pluginOptions(ctx, "cursor", []string{`{"wire_trace": "yes"}`})
	}); err == nil || !strings.Contains(err.Error(), "true or false") {
		t.Fatalf("err = %v", err)
	}
	if !settings.Load().PluginOptions["cursor"]["wire_trace"] {
		t.Fatal("a refused value changed the switch")
	}

	// off: every switch back to the plugin's default, plugins.json emptied
	if _, err := stdoutOf(t, func() error { return pluginOptions(ctx, "cursor", []string{"off"}) }); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().PluginOptions["cursor"]; len(got) != 0 {
		t.Fatalf("after off PluginOptions = %v", got)
	}
	if o := entryOptions(t); len(o) != 0 {
		t.Fatalf("after off plugins.json options = %v", o)
	}
}

// A switch written to plugins.json before this fix is taken out when the
// switch is set where the provider reads it.
func TestPluginOptionsMovesOldSwitch(t *testing.T) {
	optionsHome(t, `{"wire_trace":true,"keep":"me"}`)
	out, err := stdoutOf(t, func() error {
		return pluginOptions(context.Background(), "cursor", []string{`{"wire_trace": true}`})
	})
	if err != nil {
		t.Fatal(err)
	}
	if o := entryOptions(t); len(o) != 1 || o["keep"] != "me" {
		t.Fatalf("plugins.json options = %v; said %q", o, out)
	}
	if !settings.Load().PluginOptions["cursor"]["wire_trace"] {
		t.Fatal("switch not set")
	}
}
