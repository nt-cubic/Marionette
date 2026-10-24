package gui

import (
	"context"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gateway"
)

// The Codex app left unable to send on its used-up ChatGPT account is
// reopened by magpie on macOS (MasterYi on Discord), once each time the
// account runs out, and only while: the app open then still runs, magpie
// is Codex's provider for the account, the account is out, and no turn is
// in flight through magpie or ended in the last codexAppOutIdle. Nothing
// real is quit: the reopen is a stub.
func TestCodexAppOutReopenedOnce(t *testing.T) {
	var (
		user            = "me@example.com"
		tried, agentOut = false, true
		usedUp          = true
		busy            gateway.Busy
		bundles         = []string{"/Applications/ChatGPT.app"}
		goos            = "darwin"
		reopened        []string
	)
	saved := []any{codexAppOutUser, codexAppOutTried, codexAppOutOnce, codexAppOutUsedUp, codexAgentOut, codexAppOutBundles, codexAppOutReopen, gatewayBusy, codexAppOutOS}
	t.Cleanup(func() {
		codexAppOutUser, codexAppOutTried, codexAppOutOnce = saved[0].(func() string), saved[1].(func() bool), saved[2].(func() bool)
		codexAppOutUsedUp, codexAgentOut, codexAppOutBundles = saved[3].(func() bool), saved[4].(func() bool), saved[5].(func() []string)
		codexAppOutReopen, gatewayBusy, codexAppOutOS = saved[6].(func(context.Context, string) error), saved[7].(func() gateway.Busy), saved[8].(string)
	})
	codexAppOutUser = func() string { return user }
	codexAppOutTried = func() bool { return tried }
	codexAppOutOnce = func() bool {
		if tried {
			return false
		}
		tried = true
		return true
	}
	codexAppOutUsedUp = func() bool { return usedUp }
	codexAgentOut = func() bool { return agentOut }
	codexAppOutBundles = func() []string { return bundles }
	codexAppOutReopen = func(_ context.Context, b string) error { reopened = append(reopened, b); return nil }
	gatewayBusy = func() gateway.Busy { return busy }
	codexAppOutOS = goos
	now := time.Now()

	check := func(when string, want bool) {
		t.Helper()
		if got := reopenCodexAppOut(context.Background(), now); got != want {
			t.Fatalf("%s: tried %v, want %v (reopened %v)", when, got, want, reopened)
		}
	}
	if s := codexAppOutState(); s == nil || s.User != user || !s.Reopening {
		t.Fatalf("the notice before: %+v", s)
	}
	busy = gateway.Busy{Requests: 1}
	check("a turn in flight", false)
	busy = gateway.Busy{Last: now.Add(-10 * time.Second)}
	check("a turn ended 10s ago", false)
	busy = gateway.Busy{Last: now.Add(-codexAppOutIdle)}
	agentOut = false
	check("magpie not Codex's provider for it", false)
	agentOut, usedUp = true, false
	check("the account has room", false)
	usedUp, user = true, ""
	check("no app open as it ran out", false)
	user, bundles = "me@example.com", nil
	check("no app open from before the change", false)
	bundles = []string{"/Applications/ChatGPT.app"}
	codexAppOutOS = "linux"
	check("not macOS", false)
	if s := codexAppOutState(); s == nil || s.Reopening {
		t.Fatalf("the notice off macOS: %+v", s)
	}
	codexAppOutOS = "darwin"
	if len(reopened) != 0 {
		t.Fatalf("reopened before every condition held: %v", reopened)
	}
	check("all hold", true)
	if len(reopened) != 1 || reopened[0] != "/Applications/ChatGPT.app" {
		t.Fatalf("reopened %v", reopened)
	}
	check("again, the same time out", false)
	if len(reopened) != 1 {
		t.Fatalf("reopened twice: %v", reopened)
	}
	if s := codexAppOutState(); s == nil || s.Reopening {
		t.Fatalf("the notice after a reopen that didn't take: %+v", s)
	}
}
