package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// dbydd on Discord: an in-order group whose first members are relays
// (sub2api) that are up, and whose last is a Codex subscription. Once a
// conversation has landed on Codex (its relays failed for a moment), and
// Codex's quota then answers 429, the request goes on to the relays, the
// agent never sees the 429, and the conversation stays off Codex.
func TestGroupLastMemberSpentGoesBackToEarlier(t *testing.T) {
	for _, x := range []struct {
		name   string
		refuse func(w http.ResponseWriter)
	}{
		{"usage_limit_reached", func(w http.ResponseWriter) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(429)
			io.WriteString(w, `{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":7200}}`)
		}},
		{"rate_limit_exceeded", func(w http.ResponseWriter) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(429)
			io.WriteString(w, `{"error":{"type":"rate_limit_exceeded","message":"Rate limit reached. Please try again later."}}`)
		}},
		{"bare 429", func(w http.ResponseWriter) {
			w.WriteHeader(429)
		}},
	} {
		t.Run(x.name, func(t *testing.T) {
			codexSignedIn(t)
			sticks.Lock()
			sticks.m = map[string]stick{}
			sticks.Unlock()
			// the ChatGPT backend: answers until the quota is gone
			var mu sync.Mutex
			spent, codexN := false, 0
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				io.ReadAll(r.Body)
				mu.Lock()
				codexN++
				out := spent
				mu.Unlock()
				if out {
					x.refuse(w)
					return
				}
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, sse(
					`data: {"type":"response.created","response":{"id":"r1","model":"gpt-5.5"}}`,
					`data: {"type":"response.output_text.delta","delta":"pong"}`,
					`data: {"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":7000,"output_tokens":1,"input_tokens_details":{"cached_tokens":6000}}}}`))
			}))
			t.Cleanup(up.Close)
			was := provider.CodexBase
			provider.CodexBase = up.URL + "/backend-api/codex"
			t.Cleanup(func() { provider.CodexBase = was })
			// the relay: down for a moment, then up
			relayDown, relayN := true, 0
			relay := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				io.ReadAll(r.Body)
				mu.Lock()
				relayN++
				down := relayDown
				mu.Unlock()
				if down {
					w.Header().Set("Content-Type", "application/json")
					w.WriteHeader(503)
					io.WriteString(w, `{"error":{"message":"upstream busy"}}`)
					return
				}
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, grokAnswer)
			}))
			t.Cleanup(relay.Close)
			if err := provider.Save(provider.Provider{ID: "xai", Name: "XAI", Key: "k", Models: []string{"m"}, Chat: relay.URL + "/v1"}); err != nil {
				t.Fatal(err)
			}
			refusalGroup(t, "xai/m", "codex/gpt-5.5")
			s := New()
			ask := func() (int, string) {
				rec := httptest.NewRecorder()
				req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(codexAsk))
				req.Header.Set("Authorization", "Bearer chatgpt-token")
				req.Header.Set("chatgpt-account-id", "acct-1")
				req.Header.Set("session_id", "thread-dbydd")
				s.Handler().ServeHTTP(rec, req)
				return rec.Code, rec.Body.String()
			}
			// the relay busy: Codex answers, and the conversation is on it
			for turn := range 2 {
				if code, body := ask(); code != 200 || !strings.Contains(body, "pong") {
					t.Fatalf("turn %d on Codex: %d %s (%+v)", turn, code, body, lastRoute(s).Tries)
				}
			}
			// the relay is up again, and Codex's quota is gone
			mu.Lock()
			spent, relayDown = true, false
			codexN = 0
			mu.Unlock()
			for turn := range 4 {
				code, body := ask()
				if code != 200 || !strings.Contains(body, "hello") {
					r := lastRoute(s)
					t.Fatalf("turn %d after the quota ran out: %d %s\naffinity %+v\ntries %+v", turn, code, body, r.Affinity, r.Tries)
				}
			}
			mu.Lock()
			defer mu.Unlock()
			if codexN > 1 {
				t.Fatalf("Codex asked %d times once out (relay %d); it is asked once at most", codexN, relayN)
			}
		})
	}
}

// The same group with the relay failing now and then, as relays do: a
// request it fails goes on to Codex, out of its quota, and then back to
// the relay, which answers; the agent never sees Codex's 429 nor the
// relay's moment of failure. Out of its quota, Codex rests, and the
// relay's next failure has it asked again rather than Codex. Rate
// limited for a moment, it is asked as a resting member is (probeOne).
func TestGroupRelayBlipDoesNotEndOnSpentLast(t *testing.T) {
	for _, x := range []struct {
		name   string
		spent  bool // out of its quota, not limited for a moment
		refuse func(w http.ResponseWriter)
	}{
		{"usage_limit_reached", true, func(w http.ResponseWriter) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(429)
			io.WriteString(w, `{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":7200}}`)
		}},
		{"rate_limit_exceeded", false, func(w http.ResponseWriter) {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(429)
			io.WriteString(w, `{"error":{"type":"rate_limit_exceeded","message":"Rate limit reached. Please try again later."}}`)
		}},
		{"bare 429", false, func(w http.ResponseWriter) {
			w.WriteHeader(429)
		}},
	} {
		for _, blip := range []int{503, 502, 429, 500} {
			t.Run(x.name+" "+http.StatusText(blip), func(t *testing.T) {
				codexSignedIn(t)
				sticks.Lock()
				sticks.m = map[string]stick{}
				sticks.Unlock()
				var mu sync.Mutex
				codexN, relayN := 0, 0
				up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					io.ReadAll(r.Body)
					mu.Lock()
					codexN++
					mu.Unlock()
					x.refuse(w)
				}))
				t.Cleanup(up.Close)
				was := provider.CodexBase
				provider.CodexBase = up.URL + "/backend-api/codex"
				t.Cleanup(func() { provider.CodexBase = was })
				relay := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					io.ReadAll(r.Body)
					mu.Lock()
					relayN++
					fail := relayN%2 == 1
					mu.Unlock()
					if fail {
						w.Header().Set("Content-Type", "application/json")
						w.WriteHeader(blip)
						io.WriteString(w, `{"error":{"message":"upstream error"}}`)
						return
					}
					w.Header().Set("Content-Type", "text/event-stream")
					io.WriteString(w, grokAnswer)
				}))
				t.Cleanup(relay.Close)
				if err := provider.Save(provider.Provider{ID: "xai", Name: "XAI", Key: "k", Models: []string{"m"}, Chat: relay.URL + "/v1"}); err != nil {
					t.Fatal(err)
				}
				refusalGroup(t, "xai/m", "codex/gpt-5.5")
				s := New()
				for turn := range 4 {
					rec := httptest.NewRecorder()
					req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(codexAsk))
					req.Header.Set("Authorization", "Bearer chatgpt-token")
					req.Header.Set("chatgpt-account-id", "acct-1")
					req.Header.Set("session_id", "thread-dbydd")
					s.Handler().ServeHTTP(rec, req)
					if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello") {
						r := lastRoute(s)
						t.Fatalf("turn %d: %d %s\naffinity %+v\ntries %+v", turn, rec.Code, rec.Body.String(), r.Affinity, r.Tries)
					}
				}
				mu.Lock()
				defer mu.Unlock()
				if x.spent && codexN > 1 {
					t.Fatalf("Codex asked %d times (relay %d); out of its quota, it is asked once", codexN, relayN)
				}
			})
		}
	}
}

// With the group's Patience off, nobody is asked again within a request:
// the request the relay failed has the agent told so. Codex, out of its
// quota as the last one asked, rests all the same, as a key does (#1772),
// and the relay's next failure has the relay asked again, not Codex.
func TestGroupSpentLastAccountRests(t *testing.T) {
	codexSignedIn(t)
	sticks.Lock()
	sticks.m = map[string]stick{}
	sticks.Unlock()
	var mu sync.Mutex
	codexN, relayN := 0, 0
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		mu.Lock()
		codexN++
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(429)
		io.WriteString(w, `{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_in_seconds":7200}}`)
	}))
	t.Cleanup(up.Close)
	was := provider.CodexBase
	provider.CodexBase = up.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })
	relay := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		mu.Lock()
		relayN++
		fail := relayN%2 == 1
		mu.Unlock()
		if fail {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(503)
			io.WriteString(w, `{"error":{"message":"upstream error"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, grokAnswer)
	}))
	t.Cleanup(relay.Close)
	if err := provider.Save(provider.Provider{ID: "xai", Name: "XAI", Key: "k", Models: []string{"m"}, Chat: relay.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"xai/m", "codex/gpt-5.5"}, Routing: provider.Ordered, Patience: provider.PatienceOff}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for turn := range 5 {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(codexAsk))
		req.Header.Set("Authorization", "Bearer chatgpt-token")
		req.Header.Set("chatgpt-account-id", "acct-1")
		req.Header.Set("session_id", "thread-dbydd")
		s.Handler().ServeHTTP(rec, req)
		if turn > 0 && (rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello")) {
			r := lastRoute(s)
			t.Fatalf("turn %d: %d %s\ntries %+v", turn, rec.Code, rec.Body.String(), r.Tries)
		}
	}
	mu.Lock()
	defer mu.Unlock()
	if codexN > 1 {
		t.Fatalf("Codex asked %d times (relay %d); out of its quota, it is asked once", codexN, relayN)
	}
}
