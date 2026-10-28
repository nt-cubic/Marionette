package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

// recordingProxy is an HTTP proxy that answers each request itself, as the
// vendor would, and tells what it was asked.
func recordingProxy(t *testing.T) (*httptest.Server, func() []string) {
	t.Helper()
	var mu sync.Mutex
	var seen []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r.URL.String())
		mu.Unlock()
		w.Write([]byte("{}"))
	}))
	t.Cleanup(srv.Close)
	return srv, func() []string {
		mu.Lock()
		defer mu.Unlock()
		return slices.Clone(seen)
	}
}

// #2777 (zRain on Discord): a plugin's check-in goes through the proxy its
// usage goes through: the account's own, else its provider's, and never
// the global one past them. With the provider set to direct and the
// global proxy a residential entry the vendor refused, the balance read
// but each check-in, the loop's and a press, went out through the global
// proxy (ECONNRESET).
func TestPluginCheckinGoesThroughItsProxy(t *testing.T) {
	bun := testenv.Bun(t)
	home := claudeHome(t)
	for _, k := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"} {
		t.Setenv(k, "")
	}
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("FAKE_CHECKIN", filepath.Join(home, "presses"))
	// a name no request reaches but through a proxy that answers for it
	t.Setenv("FAKE_CHECKIN_URL", "http://vendor.invalid/checkin")
	t.Cleanup(plugin.Settle)
	global, globalSaw := recordingProxy(t)
	own, ownSaw := recordingProxy(t)
	// the global proxy, set before the host starts, as the user's is
	s := settings.Load()
	s.Proxy = global.URL
	s.PluginCheckins = map[string]bool{"fakeco": true}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	for _, k := range []string{"ck-direct", "ck-own"} {
		if _, err := PluginAPIKey(ctx, "fakeco", 0, nil, k); err != nil {
			t.Fatal(err)
		}
	}
	ps := pluginCheckinProviders()
	if len(ps) != 1 || len(ps[0].accounts) != 2 {
		t.Fatalf("the plugins that check in: %+v", ps)
	}
	// the provider direct, one account through a proxy of its own
	auths := plugin.Auths("fakeco")
	var ownUser, directUser string
	for _, l := range pluginLogins(mustPlugin(t)) {
		switch auths[l.acct.Key]["key"] {
		case "ck-own":
			ownUser = l.User
		case "ck-direct":
			directUser = l.User
		}
	}
	if ownUser == "" || directUser == "" || ownUser == directUser {
		t.Fatalf("the accounts: %q, %q (%+v)", ownUser, directUser, ps[0].accounts)
	}
	setProxies(t, PluginID("fakeco"), "direct", map[string]string{accountKey(ownUser): own.URL})

	check := func(name string, rs []WorkBuddyCheckin) {
		t.Helper()
		if len(rs) != 2 {
			t.Fatalf("%s: %+v", name, rs)
		}
		for _, r := range rs {
			switch r.User {
			case ownUser:
				if r.Outcome != CheckinClaimed {
					t.Errorf("%s: the account with its own proxy: %+v", name, r)
				}
			case directUser:
				// sent direct: vendor.invalid is reached by no one
				if r.Outcome != CheckinFailed {
					t.Errorf("%s: the direct account: %+v", name, r)
				}
			default:
				t.Errorf("%s: an answer for %q: %+v", name, r.User, r)
			}
		}
		if g := globalSaw(); len(g) != 0 {
			t.Errorf("%s: the global proxy was asked %q", name, g)
		}
	}

	// the loop's, on a day of its own
	day := time.Date(2026, 10, 6, 10, 0, 0, 0, beijing)
	loop := pluginCheckiner{now: func() time.Time { return day }, only: func(id string) bool { return PluginCheckinOn(settings.Load(), id) }}
	check("the loop", loop.checkinNow(ctx, false))
	if o := ownSaw(); !slices.Equal(o, []string{"http://vendor.invalid/checkin/ck-own"}) {
		t.Fatalf("the loop: the account's proxy was asked %q", o)
	}
	// a press, today
	check("a press", CheckInPlugins(ctx, "fakeco"))
	if o := ownSaw(); !slices.Equal(o, []string{"http://vendor.invalid/checkin/ck-own", "http://vendor.invalid/checkin/ck-own"}) {
		t.Fatalf("a press: the account's proxy was asked %q", o)
	}
}
