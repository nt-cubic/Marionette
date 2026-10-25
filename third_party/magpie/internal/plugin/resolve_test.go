package plugin

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// testRegistry is the community's registry.json as these tests serve it:
// the real file's shape, a provider plugin with two ids, a middleware, and
// two plugins that share a provider id.
const testRegistry = `{"version":1,"plugins":[
{"package":"@magpie-community/opencode-workbuddy-auth","name":"WorkBuddy","providers":["workbuddy","workbuddy-ai"],"replaces":"workbuddy","community":true},
{"package":"@magpie-community/opencode-qoder-auth","name":"Qoder","providers":["qoder"],"replaces":"qoder","community":true},
{"package":"@magpie-community/opencode-commandcode-auth","name":"Command Code","providers":["commandcode-plan"],"replaces":"commandcode-plan","community":true},
{"package":"@magpie-community/opencode-zed-auth","name":"Zed","providers":["zed"],"replaces":"zed","community":true},
{"package":"@magpie-community/middleware-think-tags","name":"Think Tags","kind":"middleware","community":true},
{"package":"@magpie-community/opencode-twin-a-auth","name":"Twin A","providers":["twin"],"community":true},
{"package":"@magpie-community/opencode-twin-b-auth","name":"Twin B","providers":["twin"],"community":true}
]}`

// serveRegistry has the market read testRegistry from a server of the
// test's own, or fail to when down is set; nothing is kept between tests.
func serveRegistry(t *testing.T, down *bool) {
	t.Helper()
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if down != nil && *down {
			http.Error(w, "down", http.StatusBadGateway)
			return
		}
		w.Write([]byte(testRegistry))
	}))
	t.Cleanup(srv.Close)
	t.Setenv("MAGPIE_PLUGIN_MARKET", srv.URL)
	forgetMarket := func() { marketMu.Lock(); marketList, marketErr = nil, nil; marketMu.Unlock() }
	forgetMarket()
	t.Cleanup(forgetMarket)
	old := SubscriptionPlugin
	SubscriptionPlugin = func(id string) string {
		if id == "qoder-cn" {
			return "@magpie-community/opencode-qoder-auth"
		}
		return ""
	}
	t.Cleanup(func() { SubscriptionPlugin = old })
}

// A community plugin's short or shown name, case aside, is its package
// (Atlas on Discord: `magpie plugin add WorkBuddy` said it "isn't an npm
// package name", and a bare workbuddy would have installed npm's unscoped
// workbuddy). npm's own package names, scoped ones, paths and git specs
// are kept as written; npm: asks for npm's package of a name a community
// plugin goes by.
func TestResolveCommunityNames(t *testing.T) {
	serveRegistry(t, nil)
	ctx := context.Background()
	for in, want := range map[string]string{
		"workbuddy":                 "@magpie-community/opencode-workbuddy-auth",
		"WorkBuddy":                 "@magpie-community/opencode-workbuddy-auth",
		" WORKBUDDY ":               "@magpie-community/opencode-workbuddy-auth",
		"workbuddy-ai":              "@magpie-community/opencode-workbuddy-auth",
		"opencode-workbuddy-auth":   "@magpie-community/opencode-workbuddy-auth",
		"workbuddy@0.1.3":           "@magpie-community/opencode-workbuddy-auth@0.1.3",
		"Qoder":                     "@magpie-community/opencode-qoder-auth",
		"qoder-cn":                  "@magpie-community/opencode-qoder-auth", // a mover's id, not in the registry
		"think-tags":                "@magpie-community/middleware-think-tags",
		"Think Tags":                "@magpie-community/middleware-think-tags",
		"Command Code":              "@magpie-community/opencode-commandcode-auth",
		"commandcode":               "@magpie-community/opencode-commandcode-auth",
		"commandcode-plan":          "@magpie-community/opencode-commandcode-auth",
		"zed":                       "@magpie-community/opencode-zed-auth",
		"opencode-gemini-auth":      "opencode-gemini-auth",
		"opencode-gemini-auth@1.2":  "opencode-gemini-auth@1.2",
		"@acme/workbuddy":           "@acme/workbuddy",
		"npm:think-tags":            "think-tags",
		"npm:workbuddy@2.0.0":       "workbuddy@2.0.0",
		"github:someone/workbuddy":  "github:someone/workbuddy",
		"./workbuddy":               "./workbuddy",
		"@magpie-community/zed-new": "@magpie-community/zed-new",
	} {
		got, err := resolve(ctx, strings.TrimSpace(in))
		if err != nil || got != want {
			t.Errorf("resolve(%q) = %q, %v; want %q", in, got, err, want)
		}
	}
}

// A name that two plugins go by, or none and isn't npm's, is refused with
// the plugins it could be; never sent to npm.
func TestResolveRefusesWhatItCantName(t *testing.T) {
	serveRegistry(t, nil)
	ctx := context.Background()
	_, err := resolve(ctx, "twin")
	if err == nil || !strings.Contains(err.Error(), "opencode-twin-a-auth") || !strings.Contains(err.Error(), "opencode-twin-b-auth") {
		t.Fatalf("twin: %v, want both candidates named", err)
	}
	_, err = resolve(ctx, "Work Buddy Pro")
	if err == nil || !strings.Contains(err.Error(), "workbuddy, zed") || !strings.Contains(err.Error(), "think-tags (Think Tags)") {
		t.Fatalf("Work Buddy Pro: %v, want the community plugins listed", err)
	}
	if _, err := resolve(ctx, "npm:Not A Name"); err == nil {
		t.Fatal("npm:Not A Name: no error")
	}
}

// The list unread is unknown, not empty: a name it would have to rule out
// isn't sent to npm on a guess, while one a copy magpie has names is still
// its package, and npm: is still npm's.
func TestResolveListUnread(t *testing.T) {
	down := true
	serveRegistry(t, &down)
	ctx := context.Background()
	_, err := resolve(ctx, "opencode-gemini-auth")
	if err == nil || !strings.Contains(err.Error(), "couldn't read the community plugin list") || !strings.Contains(err.Error(), "npm:opencode-gemini-auth") {
		t.Fatalf("list down: %v, want it refused, saying why and npm:", err)
	}
	// the copy built in names WorkBuddy
	if got, err := resolve(ctx, "WorkBuddy"); err != nil || got != "@magpie-community/opencode-workbuddy-auth" {
		t.Fatalf("list down, WorkBuddy: %q, %v", got, err)
	}
	if got, err := resolve(ctx, "npm:opencode-gemini-auth"); err != nil || got != "opencode-gemini-auth" {
		t.Fatalf("list down, npm:: %q, %v", got, err)
	}
	// up again: read anew once the held copy's minute is over
	down = false
	marketMu.Lock()
	marketAt = time.Time{}
	marketMu.Unlock()
	if got, err := resolve(ctx, "opencode-gemini-auth"); err != nil || got != "opencode-gemini-auth" {
		t.Fatalf("list up: %q, %v", got, err)
	}
}

// Add is where every route installs by name (the CLI, the Plugins page's
// field, a backup's plugins): WorkBuddy installs the community package,
// and bun is never asked for npm's unscoped workbuddy.
func TestAddByCommunityName(t *testing.T) {
	for _, in := range []string{"WorkBuddy", "workbuddy"} {
		t.Run(in, func(t *testing.T) {
			dir := offSandbox(t)
			serveRegistry(t, nil)
			ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
			defer cancel()
			e, err := Add(ctx, in)
			if err != nil {
				t.Fatal(err)
			}
			const want = "@magpie-community/opencode-workbuddy-auth@latest"
			if e.Spec != want {
				t.Fatalf("Add(%q) = %s, want %s", in, e.Spec, want)
			}
			b, _ := os.ReadFile(filepath.Join(dir, "bun.log"))
			if !strings.Contains(string(b), "add --ignore-scripts "+want) || strings.Contains(string(b), " workbuddy@") {
				t.Fatalf("bun was asked:\n%s", b)
			}
		})
	}
}
