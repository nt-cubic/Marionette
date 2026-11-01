package gateway

import (
	"net/http"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A middleware's onAccount passing the first of two Codex accounts over
// (bm_rabbit's MOD_Probe): the request goes to the second, the first is
// never sent it, and nothing is held against the first, which the next
// request is asked of again.
func TestOnAccountSkipMovesToNextAccount(t *testing.T) {
	codexSignedIn(t, "spare@example.com")
	if err := provider.SetRouting("codex", provider.Ordered); err != nil {
		t.Fatal(err)
	}
	var tried []string
	chatgptRefusing(t, &tried, func(w http.ResponseWriter) { t.Error("acct-1 refused stand-in reached") })
	onMiddleware(t, `
export function onAccount(account, ctx) {
  if (ctx.state.passed) return;
  ctx.state.passed = account.ref;
  return {skip: true};
}`)
	s := New()
	code, body := codexAskOn(s, `{"model":"gpt-5.5","stream":true,"input":"ping"}`)
	if code != 200 || !strings.Contains(body, "pong") {
		t.Fatalf("status %d: %s", code, body)
	}
	if strings.Join(tried, ",") != "acct-2" {
		t.Fatalf("tried %v, not the second account alone", tried)
	}
	if r, ok := restOf("codex@me@example.com"); ok {
		t.Fatalf("the account passed over rests: %+v", r)
	}
	restingUntil.Lock()
	n := len(restingUntil.m)
	restingUntil.Unlock()
	if n != 0 {
		t.Fatalf("%d accounts rest", n)
	}
	// the next request passes the first over again, by its ref: still
	// nothing against it, and the turn goes on to the second as before
	tried = nil
	if code, body := codexAskOn(s, `{"model":"gpt-5.5","stream":true,"input":"ping"}`); code != 200 || strings.Join(tried, ",") != "acct-2" {
		t.Fatalf("second request: %d %s tried %v", code, body, tried)
	}
}

// Every account passed over: the agent is told none is ready, as with no
// account to ask, and none is sent the request (no fail-open) or rests.
func TestOnAccountSkipEveryAccount(t *testing.T) {
	codexSignedIn(t, "spare@example.com")
	if err := provider.SetRouting("codex", provider.Ordered); err != nil {
		t.Fatal(err)
	}
	var tried []string
	chatgptRefusing(t, &tried, func(w http.ResponseWriter) {})
	onMiddleware(t, `export function onAccount() { return {skip: true}; }`)
	code, body := codexAskOn(New(), `{"model":"gpt-5.5","stream":true,"input":"ping"}`)
	if code != 404 || !strings.Contains(body, "gpt-5.5's models is ready") {
		t.Fatalf("status %d: %s", code, body)
	}
	if len(tried) != 0 {
		t.Fatalf("sent to %v though every account was passed over", tried)
	}
	for _, id := range []string{"codex@me@example.com", "codex@spare@example.com"} {
		if r, ok := restOf(id); ok {
			t.Fatalf("%s rests: %+v", id, r)
		}
	}
}
