package provider

import (
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// Both WorkBuddy builds (the CodeBuddy plan) refuse a chat whose system
// prompt is Codex's or Claude Code's as "from an unapproved channel"
// (#182); the error says what to do about it, and other errors don't.
func TestWorkBuddyRefusedHint(t *testing.T) {
	home := signIn(t)
	future := time.Now().Add(24 * time.Hour).UnixMilli()
	for _, f := range []string{wbAuthFile(home), filepath.Join(filepath.Dir(wbAuthFile(home)), "workbuddy-desktop-ai.info")} {
		writeFile(t, f, map[string]any{
			"account": map[string]any{"uid": "u1", "nickname": "me"},
			"auth":    map[string]any{"accessToken": "a", "refreshToken": "r", "expiresAt": future, "refreshExpiresAt": future},
		})
	}
	refused := []byte(`{"code":11101,"msg":"Illegal API invocation from an unapproved channel"}`)
	for _, id := range []string{"workbuddy", WorkBuddyAIID} {
		p, ok := find(All(), id)
		if !ok {
			t.Fatalf("%s not signed in", id)
		}
		if got := p.Explain(p.Name+": Illegal API invocation from an unapproved channel", 400, refused); !strings.HasSuffix(got, " — "+WBRefusedHint) {
			t.Errorf("%s: %q", id, got)
		}
		if got := p.Explain(p.Name+": bad", 400, []byte(`{"code":1,"msg":"first message is not system prompt"}`)); got != p.Name+": bad" {
			t.Errorf("%s, another error: %q", id, got)
		}
		// an account WorkBuddy restricted (#1849), its 403 as it came
		restricted := []byte(`{"code":11140,"msg":"request illegal","displayMsg":{"en":"Your account has been restricted due to unusual activity","zh":"账号存在异常行为，已被限制使用"}}`)
		if got := p.Explain(p.Name+": request illegal", 403, restricted); got != p.Name+": request illegal — "+WBRestrictedHint {
			t.Errorf("%s, restricted: %q", id, got)
		}
		if got := p.Explain(p.Name+": Illegal API invocation from an unapproved channel", 400, refused); strings.Contains(got, WBRestrictedHint) {
			t.Errorf("%s: 11128 told as restricted: %q", id, got)
		}
	}
}
