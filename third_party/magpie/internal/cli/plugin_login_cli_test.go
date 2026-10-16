package cli

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// magpie accounts add, signing in to a plugin's provider removed from
// magpie, brings it back, as the window's sign-in and a built-in's do.
func TestAccountsAddBringsBackARemovedPlugin(t *testing.T) {
	ctx := fakeCoHome(t)
	if _, err := provider.PluginAPIKey(ctx, "fakeco", 0, nil, "sk-one-key-1111"); err != nil {
		t.Fatal(err)
	}
	if err := provider.Delete("fakeco"); err != nil {
		t.Fatal(err)
	}
	if _, err := provider.Find("fakeco"); err == nil {
		t.Fatal("removed provider still listed")
	}
	// the API key way (1), then the key
	piped(t, "1", "sk-two-key-2222")
	if out, err := said(t, func() error { return accountsCmd([]string{"accounts", "add", "fakeco"}) }); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	if _, err := provider.Find("fakeco"); err != nil {
		t.Fatalf("signed in again, but not listed: %v", err)
	}
}

// magpie plugin login, signing in again to an account whose sign-in the
// vendor turned away, takes its lapsed mark off, as the window's sign-in
// does, rather than the account saying "sign in again" until a request or
// a usage read goes through.
func TestPluginLoginTakesTheLapseOff(t *testing.T) {
	ctx := fakeCoHome(t)
	noBrowser(t)
	// the browser's way (2), at work (2) on the team "gone", whose usage
	// says its sign-in has expired, then the code the page shows
	login := func() {
		t.Helper()
		piped(t, "2", "2", "gone", "good")
		if out, err := said(t, func() error { return pluginCmd([]string{"plugin", "login", "fakeco"}) }); err != nil {
			t.Fatalf("%v: %s", err, out)
		}
	}
	login()
	provider.LoginUsage(ctx, "fakeco")
	if ls := provider.Logins("fakeco"); len(ls) != 1 || ls[0].Lapsed == "" {
		t.Fatalf("the vendor's refusal didn't mark the account: %+v", ls)
	}
	login()
	if ls := provider.Logins("fakeco"); len(ls) != 1 || ls[0].Lapsed != "" {
		t.Fatalf("signed in again, but still lapsed: %+v", ls)
	}
}

// An account signed in again is read afresh: the reading kept from the
// sign-in the vendor refused doesn't answer for the new one, saying its
// sign-in has expired for up to a minute after it was renewed.
func TestPluginLoginReadsUsageAfresh(t *testing.T) {
	var status atomic.Int32
	status.Store(http.StatusUnauthorized)
	vendor := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(int(status.Load()))
		fmt.Fprint(w, "Fake Pro")
	}))
	defer vendor.Close()
	t.Setenv("FAKE_USAGE", vendor.URL)
	ctx := fakeCoHome(t)
	noBrowser(t)
	login := func() {
		t.Helper()
		piped(t, "2", "2", "team", "good")
		if out, err := said(t, func() error { return pluginCmd([]string{"plugin", "login", "fakeco"}) }); err != nil {
			t.Fatalf("%v: %s", err, out)
		}
	}
	login()
	if q := provider.LoginUsage(ctx, "fakeco")["team@fake"]; q.Error == "" {
		t.Fatalf("the vendor's 401 read clean: %+v", q)
	}
	status.Store(http.StatusOK)
	login()
	if q := provider.LoginUsage(ctx, "fakeco")["team@fake"]; q.Error != "" || q.Plan != "Fake Pro" {
		t.Fatalf("signed in again, but the usage is the old sign-in's: %+v", q)
	}
}

// fakeCoHome is a HOME of the test's own with the plugin host's test
// plugin (FakeCo) added, as TestAccountsOfAPlugin sets one up.
func fakeCoHome(t *testing.T) context.Context {
	t.Helper()
	bun := testenv.Bun(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("CLAUDE_CONFIG_DIR", filepath.Join(home, ".claude"))
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	return ctx
}

// noBrowser has the sign-in pages magpie opens go nowhere. It swaps the
// opener rather than putting a do-nothing open first on PATH: a script
// written now is a program macOS checks on its first run, which took 25 to
// 55 seconds while other test binaries ran, and ate the minute the plugin
// calls have (#1284).
func noBrowser(t *testing.T) {
	t.Helper()
	old := openInBrowser
	openInBrowser = func(string) {}
	t.Cleanup(func() { openInBrowser = old })
}

// magpie plugin login installs the CLI a plugin signs in through before
// the sign-in, as the window's does, and a failed install stops it there
// rather than the plugin saying "install Grok Build first: curl … | bash",
// which magpie's Docker image has no curl to run (#2223, evier12345).
func TestPluginLoginInstallsItsCLIFirst(t *testing.T) {
	fakeCoHome(t)
	noBrowser(t)
	var installed []string
	missing := "Fake CLI"
	fail := error(nil)
	old := pluginSignInCLI
	pluginSignInCLI.missing = func(id string) string { return missing }
	pluginSignInCLI.install = func(ctx context.Context, id string) error {
		installed = append(installed, id)
		if fail == nil {
			missing = ""
		}
		return fail
	}
	t.Cleanup(func() { pluginSignInCLI = old })

	fail = fmt.Errorf("installing Fake CLI failed")
	piped(t, "2", "2", "team", "good")
	if out, err := said(t, func() error { return pluginCmd([]string{"plugin", "login", "fakeco"}) }); err == nil || err != fail {
		t.Fatalf("a failed install went on: %v: %s", err, out)
	}
	if ls := provider.Logins("fakeco"); len(ls) != 0 {
		t.Fatalf("signed in without its CLI: %+v", ls)
	}
	fail = nil
	piped(t, "2", "2", "team", "good")
	if out, err := said(t, func() error { return pluginCmd([]string{"plugin", "login", "fakeco"}) }); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	if len(installed) != 2 || installed[1] != "fakeco" {
		t.Fatalf("installed %v", installed)
	}
	if ls := provider.Logins("fakeco"); len(ls) != 1 {
		t.Fatalf("not signed in after the install: %+v", ls)
	}
	// an API key's way runs no CLI: nothing is installed for it
	missing = "Fake CLI"
	piped(t, "1", "sk-one-key-1111")
	if out, err := said(t, func() error { return pluginCmd([]string{"plugin", "login", "fakeco"}) }); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	if len(installed) != 2 {
		t.Fatalf("installed for a key: %v", installed)
	}
}
