package plugin

import (
	"context"
	"net/url"
	"strings"
	"testing"
	"time"
)

// With only a SOCKS5 proxy, which a Mac's system SOCKS proxy hands bun
// through netproxy.Env, a plugin still installs (#1409): bun takes only
// http:// and https:// proxies, and every registry fetch failed as
// UnsupportedProxyProtocol. bunCommand bridges it, for add, update and
// remove alike, and the install goes through the SOCKS proxy.
func TestInstallThroughSOCKSProxy(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-socks-auth"
	heldRegistry(t, pkg, map[string]time.Time{"0.1.0": time.Now().Add(-72 * time.Hour)}, "0.1.0")
	reg, _ := url.Parse(npmRegistry)
	socks, carried := socksTo(t, reg.Host)
	for _, k := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"} {
		t.Setenv(k, socks)
	}
	t.Setenv("NO_PROXY", "")
	t.Setenv("no_proxy", "")

	for _, kv := range bunCommand(context.Background(), "bun", t.TempDir(), "add", "x").Env {
		if k, v, _ := strings.Cut(kv, "="); strings.HasSuffix(strings.ToUpper(k), "_PROXY") && strings.HasPrefix(v, "socks") {
			t.Fatalf("bun is given %s", kv)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()
	e, err := Add(ctx, pkg)
	if err != nil {
		t.Fatalf("add: %v", err)
	}
	if v := Version(e.Spec); v != "0.1.0" {
		t.Fatalf("installed %q", v)
	}
	if carried.Load() == 0 {
		t.Fatal("the install didn't go through the SOCKS proxy")
	}
}
