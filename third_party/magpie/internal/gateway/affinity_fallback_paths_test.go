package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// chatTurns builds the Chat Completions body of a conversation's n-th user
// turn (n from 1), each earlier turn answered.
func chatTurns(model string, n int) string {
	var b strings.Builder
	b.WriteString(`{"model":"` + model + `","messages":[`)
	for i := 1; i <= n; i++ {
		if i > 1 {
			b.WriteString(`{"role":"assistant","content":"yo"},`)
		}
		b.WriteString(`{"role":"user","content":"turn ` + itoa(i) + `"}`)
		if i < n {
			b.WriteString(",")
		}
	}
	b.WriteString(`]}`)
	return b.String()
}

// mainReady ends every rest, as when the minute or the reset passes, and
// has the upstream answer again.
func mainReady(d *downThenUp) {
	d.mu.Lock()
	d.down = false
	d.mu.Unlock()
	restingUntil.Lock()
	for k := range restingUntil.m {
		restingUntil.m[k] = time.Now().Add(-time.Second)
	}
	restingUntil.Unlock()
}

// A provider's fallback is for when none of the provider's own keys or
// accounts can take a request (01huadalang on Discord, again on 10-31:
// "主供应商有额度后请求还是一直走备用模型，备用只是应急"). A provider routed
// In turn went round its fallbacks as if they were more of its keys: a
// new turn went to the one after the last answerer (after), which past
// the provider's last key is its fallback, and a new conversation to the
// one the fewest conversations were on (leastHeld), which is the fallback
// nobody is on — while every key of the provider was ready.
func TestRotatingProviderKeepsItsFallbackForEmergencies(t *testing.T) {
	for _, keys := range []int{1, 2} {
		t.Run(itoa(keys)+" keys", func(t *testing.T) {
			fresh(t)
			main := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}}
			spare := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}}
			mu, su := httptest.NewServer(main), httptest.NewServer(spare)
			t.Cleanup(mu.Close)
			t.Cleanup(su.Close)
			mp := provider.Provider{ID: "main", Name: "Main", Key: "main-key", Models: []string{"m"}, Anthropic: mu.URL + "/v1",
				Fallback: []string{"spare/s"}, Routing: provider.Rotate}
			if keys == 2 {
				mp.Keys = []provider.KeyAccount{{Key: "main2-key"}}
			}
			for _, p := range []provider.Provider{mp,
				{ID: "spare", Name: "Spare", Key: "spare-key", Models: []string{"s"}, Anthropic: su.URL + "/v1"},
			} {
				if err := provider.Save(p); err != nil {
					t.Fatal(err)
				}
			}
			s := New()
			for _, sess := range []string{"s1", "s2", "s3"} {
				for n := 1; n <= 4; n++ {
					code, body := postAs(t, s, sess, chatTurns("main/m", n))
					if code != 200 || strings.Contains(body, "from spare-key") || !strings.Contains(body, "from main") {
						a := s.trace.routes[len(s.trace.routes)-1].Affinity
						t.Fatalf("%s turn %d: %d %s (affinity %+v), want a key of main's while main is ready", sess, n, code, body, a)
					}
				}
			}
		})
	}
}

// The same with the provider down for one turn: the fallback answers it,
// and once the provider is ready the next turn and a new conversation go
// to the provider, in every routing and Stays but Strict.
func TestFallbackGivesBackAcrossRoutings(t *testing.T) {
	for _, c := range []struct {
		name, routing, mode string
		group               bool // the fallback is a routing group
	}{
		{"auto", "", "", false},
		{"in turn", provider.Rotate, "", false},
		{"in turn, session", provider.Rotate, provider.AffinitySession, false},
		{"least used, session", provider.LeastUsed, provider.AffinitySession, false},
		{"stays turn", "", provider.AffinityTurn, false},
		{"group fallback", "", "", true},
		{"group fallback, in turn", provider.Rotate, "", true},
		{"group fallback, session", "", provider.AffinitySession, true},
	} {
		t.Run(c.name, func(t *testing.T) {
			fresh(t)
			main := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}, down: true}
			spare := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}}
			mu, su := httptest.NewServer(main), httptest.NewServer(spare)
			t.Cleanup(mu.Close)
			t.Cleanup(su.Close)
			fb := "spare/s"
			if c.group {
				if err := provider.SaveGroup(provider.Group{ID: "fb", Name: "FB", Members: []string{"spare/s", "spare2/s"}}); err != nil {
					t.Fatal(err)
				}
				fb = provider.GroupPrefix + "fb"
			}
			for _, p := range []provider.Provider{
				{ID: "main", Name: "Main", Key: "main-key", Keys: []provider.KeyAccount{{Key: "main2-key"}}, Models: []string{"m"}, Anthropic: mu.URL + "/v1",
					Fallback: []string{fb}, Routing: c.routing, Affinity: c.mode},
				{ID: "spare", Name: "Spare", Key: "spare-key", Models: []string{"s"}, Anthropic: su.URL + "/v1"},
				{ID: "spare2", Name: "Spare2", Key: "spare2-key", Models: []string{"s"}, Anthropic: su.URL + "/v1"},
			} {
				if err := provider.Save(p); err != nil {
					t.Fatal(err)
				}
			}
			s := New()
			if code, body := postAs(t, s, "s1", chatTurns("main/m", 1)); code != 200 || !strings.Contains(body, "from spare") {
				t.Fatalf("first turn %d %s, want the fallback's answer", code, body)
			}
			mainReady(main)
			for n := 2; n <= 4; n++ {
				code, body := postAs(t, s, "s1", chatTurns("main/m", n))
				if code != 200 || !strings.Contains(body, "from main") {
					a := s.trace.routes[len(s.trace.routes)-1].Affinity
					t.Fatalf("turn %d %d %s (affinity %+v), want main's answer", n, code, body, a)
				}
			}
			for _, sess := range []string{"s2", "s3", "s4"} {
				code, body := postAs(t, s, sess, chatTurns("main/m", 1))
				if code != 200 || !strings.Contains(body, "from main") {
					a := s.trace.routes[len(s.trace.routes)-1].Affinity
					t.Fatalf("new conversation %s: %d %s (affinity %+v), want main's answer", sess, code, body, a)
				}
			}
		})
	}
}

// Stays: Strict is the user's own: a conversation it keeps on the fallback
// stays there once the provider is ready again, and the trace names the
// provider's ready key (Affinity.Main) for the Routing page to say why.
// A new conversation goes to the provider.
func TestStrictKeepsTheFallbackAndSaysTheMainIsReady(t *testing.T) {
	fresh(t)
	main := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}, down: true}
	spare := &downThenUp{cacheWriter: cacheWriter{wrote: 3000}}
	mu, su := httptest.NewServer(main), httptest.NewServer(spare)
	t.Cleanup(mu.Close)
	t.Cleanup(su.Close)
	for _, p := range []provider.Provider{
		{ID: "main", Name: "Main", Key: "main-key", Models: []string{"m"}, Anthropic: mu.URL + "/v1", Fallback: []string{"spare/s"}, Affinity: provider.AffinityStrict},
		{ID: "spare", Name: "Spare", Key: "spare-key", Models: []string{"s"}, Anthropic: su.URL + "/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	s := New()
	if code, body := postAs(t, s, "s1", chatTurns("main/m", 1)); code != 200 || !strings.Contains(body, "from spare-key") {
		t.Fatalf("first turn %d %s, want the fallback's answer", code, body)
	}
	mainReady(main)
	code, body := postAs(t, s, "s1", chatTurns("main/m", 2))
	if code != 200 || !strings.Contains(body, "from spare-key") {
		t.Fatalf("next turn %d %s, want the fallback kept (Strict)", code, body)
	}
	a := s.trace.routes[len(s.trace.routes)-1].Affinity
	if a == nil || a.Why != "strict" || a.Main == nil || a.Main.Provider != "main" {
		t.Fatalf("affinity %+v, want strict naming main as ready", a)
	}
	if code, body := postAs(t, s, "s2", chatTurns("main/m", 1)); code != 200 || !strings.Contains(body, "from main-key") {
		t.Fatalf("new conversation %d %s, want main's answer", code, body)
	}
	if a := s.trace.routes[len(s.trace.routes)-1].Affinity; a == nil || a.Main != nil {
		t.Fatalf("new conversation's affinity %+v, want no main named", a)
	}
}
