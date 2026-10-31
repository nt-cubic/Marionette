package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/settings"
)

// magpie gateway-key issue prints a short-lived key alone, for a script
// that hands it to a sandbox; --from holds it to a key let issue keys.
func TestGatewayKeyIssueCLI(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if err := settings.Save(settings.Settings{LAN: true}); err != nil {
		t.Fatal(err)
	}
	call := func(args ...string) (string, error) {
		var out bytes.Buffer
		err := gatewayKeysTo(&out, append([]string{"gateway-key"}, args...))
		return strings.TrimSpace(out.String()), err
	}
	secret, err := call("issue", "--ttl", "1h", "--name", "sandbox-1", "--models", "openai/gpt-5, openai/gpt-5-mini", "--tokens", "50000")
	if err != nil || !strings.HasPrefix(secret, access.Prefix) || strings.Contains(secret, "\n") {
		t.Fatal(secret, err)
	}
	who, ok := access.Authenticate(secret)
	if !ok || who.KeyName != "sandbox-1" || len(who.Models) != 2 || who.Limit == nil || who.Limit.Tokens != 50000 || time.Until(who.Expires) > time.Hour {
		t.Fatal(who, ok)
	}
	if got, _ := call("list"); !strings.Contains(got, "until ") {
		t.Fatal("list:", got)
	}
	for _, bad := range [][]string{{"issue"}, {"issue", "--ttl"}, {"issue", "--ttl", "soon"}, {"issue", "--ttl", "10s"}, {"issue", "--ttl", "1h", "--tokens", "-3"},
		{"issue", "--ttl", "1h", "--what"}, {"issue", "--help"}, {"issuer"}, {"issuer", "x", "maybe"}} {
		if _, err := call(bad...); err == nil {
			t.Error("accepted", bad)
		}
	}
	parent, _ := call("add", "Orchestrator")
	keys, _ := access.List()
	var pid string
	for _, k := range keys {
		if k.Name == "Orchestrator" {
			pid = k.ID
		}
	}
	if _, err := call("issue", "--ttl", "1h", "--from", pid); err == nil {
		t.Fatal("issued under a key not let issue")
	}
	if got, err := call("issuer", pid, "on"); err != nil || !strings.Contains(got, "may issue") {
		t.Fatal(got, err)
	}
	if who, _ := access.Authenticate(parent); !who.Issues {
		t.Fatal("not let issue:", who)
	}
	out, err := call("issue", "--ttl", "30m", "--from", pid, "--read-only", "--json")
	var k access.Issued
	if err != nil || json.Unmarshal([]byte(out), &k) != nil || k.Issuer != pid || !k.ReadOnly || k.Key == "" {
		t.Fatal(out, err)
	}
	if got, _ := call("list"); !strings.Contains(got, "issued by "+pid) || !strings.Contains(got, "issues keys") {
		t.Fatal("list:", got)
	}
	if got, err := call("issuer", pid, "off"); err != nil || !strings.Contains(got, "issues no keys") {
		t.Fatal(got, err)
	}
	if help := helpFor([]string{"gateway-key", "--help"}); !strings.Contains(help, "gateway-key issue --ttl") || !strings.Contains(help, "gateway-key issuer <id>") {
		t.Fatal("help:", help)
	}
}
