package gateway

import (
	"strings"
	"sync"
	"testing"
)

// A turn refused with usage_limit_reached on the account Codex is signed
// in to has that account read again at once (provider.CodexMayBeUsedUp),
// for magpie to become Codex's provider before the next five-minute look:
// the Codex app greys its Send on the account meanwhile (MasterYi on
// Discord).
func TestCodexUsedUpReplyAsksAtOnce(t *testing.T) {
	codexSignedIn(t, "spare@example.com")
	var tried, models []string
	usedUp(t, &tried, &models)
	var mu sync.Mutex
	var asked []string
	old := codexMayBeUsedUp
	codexMayBeUsedUp = func(user string) { mu.Lock(); asked = append(asked, user); mu.Unlock() }
	t.Cleanup(func() { codexMayBeUsedUp = old })

	code, body := codexPost(t, `{"model":"gpt-5.5","stream":true,"input":"ping"}`)
	if code != 200 || !strings.Contains(body, "pong") {
		t.Fatalf("status %d: %s", code, body)
	}
	mu.Lock()
	defer mu.Unlock()
	if strings.Join(asked, ",") != "me@example.com" {
		t.Fatalf("asked %v, want me@example.com", asked)
	}
}
