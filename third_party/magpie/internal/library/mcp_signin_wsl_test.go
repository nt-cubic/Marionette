package library

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/mcpauth/mcpauthtest"
)

// A WSL agent under NAT reaches the gateway at Windows' address, where a
// request without an enabled gateway key is refused with a 401, which an
// MCP client takes for the server asking it to sign in. A server magpie
// signed in to is given to it at /mcp/<name> with the key the gateway is
// shared with, as its models are; this machine's agent stays keyless, and
// so does the WSL one while nothing is shared.
func TestSignedInServerCarriesKeyBeyondLoopback(t *testing.T) {
	sandbox(t)
	root := t.TempDir()
	write(t, filepath.Join(root, "home", "me", ".claude", "settings.json"), "{}\n")
	t.Cleanup(agent.FakeWSL(map[string]string{
		"Ubuntu": "home:/home/me\ndir:.claude\nroute:default via 172.20.0.1 dev eth0\nnet:nat\n",
	}, map[string]string{"Ubuntu": root}))
	const wsl = "claude@wsl:Ubuntu"
	if err := access.ConfigureLAN(true, false); err != nil {
		t.Fatal(err)
	}
	key := access.LANSecret()
	if key == "" {
		t.Fatal("no LAN key")
	}
	f := mcpauthtest.New(t)
	ok(t)(SaveServer("", Server{Name: "neon", Transport: "http", URL: f.URL, Headers: map[string]string{"X-Team": "acme"}, Agents: []string{"claude", wsl}}))
	f.SignIn(t, "neon")
	given := func(id string) *Server {
		t.Helper()
		tg := targetByID(id)
		if tg == nil {
			t.Fatalf("%s isn't a target: %v", id, ids(Targets()))
		}
		got, err := tg.MCP.read()
		if err != nil {
			t.Fatal(err)
		}
		return got["neon"]
	}
	s := given(wsl)
	if s == nil || !strings.HasPrefix(s.URL, "http://172.20.0.1:") || !strings.HasSuffix(s.URL, "/mcp/neon") {
		t.Fatalf("%s: %+v", wsl, s)
	}
	if s.Headers["Authorization"] != "Bearer "+key || s.Headers["X-Team"] != "acme" {
		t.Fatalf("%s under NAT is given headers %v, want the LAN key as its Authorization", wsl, s.Headers)
	}
	if h := given("claude"); h == nil || h.Headers["Authorization"] != "" || h.Headers["X-Team"] != "acme" {
		t.Fatalf("this machine's claude: %+v", h)
	}

	// a new key, and sharing turned off, reach the entry without a visit
	// to the Library
	if err := access.ConfigureLAN(true, true); err != nil {
		t.Fatal(err)
	}
	if fresh := access.LANSecret(); fresh == key || given(wsl).Headers["Authorization"] != "Bearer "+fresh {
		t.Fatalf("after a new key %s is given %v", wsl, given(wsl).Headers)
	}
	if err := access.ConfigureLAN(false, false); err != nil {
		t.Fatal(err)
	}
	if s := given(wsl); s == nil || s.Headers["Authorization"] != "" {
		t.Fatalf("nothing shared, yet %s is given %+v", wsl, s)
	}
}
