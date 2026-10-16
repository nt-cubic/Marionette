package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// bareGLM asks for glm-5.3 by its bare id, as MOMO's client did, and says
// which provider the reply named and what it said. Each say is a
// conversation of its own: one answered stays where it was answered.
func bareGLM(t *testing.T, s *Server, say string) (code int, from, body string) {
	t.Helper()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"glm-5.3","messages":[{"role":"user","content":"`+say+`"}]}`))
	s.Handler().ServeHTTP(rec, req)
	return rec.Code, rec.Header().Get(providerHeader), rec.Body.String()
}

// A bare model id two providers serve (MOMO on Discord: glm-5.3 under
// opencode-go and a6api) goes where the Providers order says, every
// time, and the reply says which provider it was. With found groups on
// (the default) the group magpie found for it is what's routed, so the
// next provider answers when the first can't; off, it is the first
// provider alone.
func TestBareSlugFollowsTheProvidersOrder(t *testing.T) {
	for _, auto := range []bool{true, false} {
		t.Run(map[bool]string{true: "found groups", false: "no found groups"}[auto], func(t *testing.T) {
			fresh(t)
			provider.SetAutoGroups(auto)
			t.Cleanup(func() { provider.SetAutoGroups(true) })
			goUp, a6 := &keyed{fail: map[string]int{}}, &keyed{fail: map[string]int{}}
			serveOn(t, "opencode-go", "kg", []string{"glm-5.3"}, goUp)
			serveOn(t, "a6api", "ka", []string{"glm-5.3"}, a6)
			s := New()
			for _, say := range []string{"a", "b", "c"} {
				if code, from, body := bareGLM(t, s, say); code != http.StatusOK || from != "opencode-go" || !strings.Contains(body, "from kg") {
					t.Fatalf("%d %q %s", code, from, body)
				}
			}
			if err := provider.SetOrder([]string{"a6api", "opencode-go"}); err != nil {
				t.Fatal(err)
			}
			if code, from, body := bareGLM(t, s, "d"); code != http.StatusOK || from != "a6api" || !strings.Contains(body, "from ka") {
				t.Fatalf("reordered: %d %q %s", code, from, body)
			}
			// a conversation opencode-go answered, with its cache read,
			// stays there in the found group; with none, the order decides
			stays := map[bool]string{true: "opencode-go", false: "a6api"}[auto]
			if code, from, _ := bareGLM(t, s, "a"); code != http.StatusOK || from != stays {
				t.Fatalf("conversation a: %d %q, want %s", code, from, stays)
			}
			a6.fail["ka"] = http.StatusServiceUnavailable
			code, from, _ := bareGLM(t, s, "e")
			if auto && (code != http.StatusOK || from != "opencode-go") {
				t.Fatalf("the found group's next provider wasn't asked: %d %q", code, from)
			}
			if !auto && code == http.StatusOK {
				t.Fatalf("with no found group, %q answered", from)
			}
		})
	}
}
