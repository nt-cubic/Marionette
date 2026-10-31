package gateway

import (
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A group routed In order tries its members in the order the user put
// them in, and a conversation is kept on a later member only until the
// first can take it again: yyhhyyyyyy on Discord set an API key first and
// a Claude subscription second for Opus, and 8 of 10 Claude Code requests
// went to the subscription. One failure of the key sent a conversation to
// the subscription, and its cache kept it there at every turn after,
// the key ready. Now the next turn goes back to the key; within a turn
// it stays where the turn's reasoning was sealed, and Stays: Session, the
// user's own, keeps it too. Both healthy, every request goes to the key.
func TestOrderedGroupGivesTheConversationBackToItsFirstMember(t *testing.T) {
	for _, c := range []struct {
		name     string
		keyFails bool   // the key's first answer is a 429
		routing  string // the group's
		mode     string // the group's Stays
		want     string // who answers each of the 10 requests: k the key, s the subscription
		why      string // the affinity of the first request of turn 2
	}{
		{"both healthy", false, provider.Ordered, "", "kkkkkkkkkk", "cache"},
		{"the key failed once", true, provider.Ordered, "", "ssskkkkkkk", "order"},
		{"the key failed once, Stays: Session", true, provider.Ordered, provider.AffinitySession, "ssssssssss", "session"},
		{"the key failed once, Smart", true, "", "", "ssssssssss", "cache"},
	} {
		t.Run(c.name, func(t *testing.T) {
			claudeCaching(t)
			restingUntil.Lock()
			restingUntil.m = map[string]time.Time{}
			restingUntil.Unlock()
			sticks.Lock()
			sticks.m = map[string]stick{}
			sticks.Unlock()
			forgetRouting()
			key := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}, down: c.keyFails}
			ku := httptest.NewServer(key)
			t.Cleanup(ku.Close)
			if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "relay-key", Models: []string{"claude-opus-5-5"}, Anthropic: ku.URL + "/v1"}); err != nil {
				t.Fatal(err)
			}
			if err := provider.SaveGroup(provider.Group{ID: "opus", Name: "opus 5.5", Routing: c.routing, Affinity: c.mode,
				Members: []string{"relay/claude-opus-5-5", "claude/claude-opus-5-5"}}); err != nil {
				t.Fatal(err)
			}
			s := New()
			t.Cleanup(s.subscription.abortAll)
			h := s.Handler()
			// Claude Code's conversation: turn 1 is requests 0 to 2 (two
			// tool loops), turn 2 is 3 to 5, turn 3 is 6 to 8, turn 4 is 9
			conv := `{"role":"user","content":"fix the bug"}`
			var got strings.Builder
			for i := range 10 {
				if i == 1 && c.keyFails {
					// the key is back, its rest over
					key.mu.Lock()
					key.down = false
					key.mu.Unlock()
					restingUntil.Lock()
					for k := range restingUntil.m {
						restingUntil.m[k] = time.Now().Add(-time.Second)
					}
					restingUntil.Unlock()
				}
				body := `{"model":"group/opus","max_tokens":100,"tools":[{"name":"read","input_schema":{"type":"object"}}],"messages":[` + conv + `]}`
				r := httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body))
				r.Header.Set("x-claude-code-session-id", "s-1")
				rec := httptest.NewRecorder()
				h.ServeHTTP(rec, r)
				if rec.Code != 200 {
					t.Fatalf("request %d: %d %s", i, rec.Code, rec.Body)
				}
				switch out := rec.Body.String(); {
				case strings.Contains(out, "from relay-key"):
					got.WriteByte('k')
				case strings.Contains(out, "tok-"):
					got.WriteByte('s')
				default:
					t.Fatalf("request %d answered by nobody known: %s", i, out)
				}
				a := s.trace.routes[len(s.trace.routes)-1].Affinity
				if i == 3 && (a == nil || a.Why != c.why) {
					t.Errorf("turn 2's first request: affinity %+v, want why %q", a, c.why)
				}
				if i%3 == 2 {
					conv += `,{"role":"assistant","content":"done"},{"role":"user","content":"and ` + strconv.Itoa(i) + `"}`
				} else {
					id := "t" + strconv.Itoa(i)
					conv += `,{"role":"assistant","content":[{"type":"tool_use","id":"` + id + `","name":"read","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"` + id + `","content":"ok"}]}`
				}
			}
			if want := c.want; got.String() != want {
				t.Fatalf("answered by %s, want %s (k the key first in order, s the subscription)", got.String(), want)
			}
		})
	}
}
