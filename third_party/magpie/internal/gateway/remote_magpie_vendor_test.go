package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// fremontdango on Discord: a group of one model from two of a remote
// magpie's providers, made on the magpie that uses the remote, kept its
// conversations as if the two were one vendor, since both are the one
// remote magpie key here. The same group made on the remote itself
// doesn't. remoteVendors is the remote: a magpie serving m from two
// providers, "first/m" and "second/m", each answering "from <provider>",
// first out of its usage (429) while breaking is set.
type remoteVendors struct {
	srv      *httptest.Server
	mu       sync.Mutex
	tried    []string
	breaking bool
}

func newRemoteVendors(t *testing.T) *remoteVendors {
	rv := &remoteVendors{}
	rv.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"object":"list","data":[{"id":"first/m","object":"model"},{"id":"second/m","object":"model"},{"id":"third/m","object":"model"}]}`)
			return
		}
		var q struct {
			Model string `json:"model"`
		}
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &q)
		up, _, _ := strings.Cut(q.Model, "/")
		rv.mu.Lock()
		rv.tried = append(rv.tried, up)
		br := rv.breaking && up == "first"
		rv.mu.Unlock()
		w.Header().Set("X-Magpie-Provider", up)
		if br {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"first: usage limit reached","type":"rate_limit_error"}}`)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`data: {"id":"c2","choices":[{"index":0,"delta":{"role":"assistant","content":"from `+up+`"}}]}`,
			`data: {"id":"c2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
			`data: [DONE]`))
	}))
	t.Cleanup(rv.srv.Close)
	return rv
}

func (rv *remoteVendors) post(t *testing.T, s *Server, session string) (string, string) {
	t.Helper()
	rv.mu.Lock()
	rv.tried = nil
	rv.mu.Unlock()
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi `+session+`"}]}`))
	req.Header.Set("x-session-id", session)
	s.Handler().ServeHTTP(rec, req)
	rv.mu.Lock()
	defer rv.mu.Unlock()
	return rec.Body.String(), strings.Join(rv.tried, ",")
}

func remoteVendorsGroup(t *testing.T, rv *remoteVendors, affinity string, members ...string) {
	t.Helper()
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "sk-office", Chat: rv.srv.URL, Models: []string{"first/m", "second/m", "third/m"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: members, Routing: provider.Ordered, Affinity: affinity}); err != nil {
		t.Fatal(err)
	}
}

// Strictly kept, a conversation the remote's first provider answered stays
// on it when it runs out, its error the agent's, as the same group on the
// remote keeps it: the remote's second provider is another vendor, not another model
// of the same account, and isn't asked.
func TestRemoteMagpieStrictKeepsTheRemotesProvider(t *testing.T) {
	fresh(t)
	rv := newRemoteVendors(t)
	remoteVendorsGroup(t, rv, provider.AffinityStrict, "office/first/m", "office/second/m")
	s := New()
	if out, got := rv.post(t, s, "s1"); !strings.Contains(out, "from first") || got != "first" {
		t.Fatalf("first turn: %s tried %s", out, got)
	}
	rv.mu.Lock()
	rv.breaking = true
	rv.mu.Unlock()
	if out, got := rv.post(t, s, "s1"); !strings.Contains(out, "usage limit") || got != "first" {
		t.Fatalf("strict, its provider out: went to the remote's other provider: %s tried %s", out, got)
	}
	// resting now, it is still its error the agent gets, the other not asked
	if out, got := rv.post(t, s, "s1"); !strings.Contains(out, "usage limit") || got == "second" {
		t.Fatalf("strict, its provider resting: %s tried %s", out, got)
	}
	if why := lastWhy(t, s); why != "strict" {
		t.Fatalf("the retry traced as %q, want strict", why)
	}
}

// A conversation whose remote provider has left the group is gone, as on
// the remote, not kept on the remote's other provider as its account: the
// trace said "session", kept, where nothing of it was cached.
func TestRemoteMagpieOtherProviderIsNotTheSameAccount(t *testing.T) {
	fresh(t)
	rv := newRemoteVendors(t)
	remoteVendorsGroup(t, rv, provider.AffinitySession, "office/first/m", "office/second/m")
	s := New()
	if out, got := rv.post(t, s, "s1"); !strings.Contains(out, "from first") || got != "first" {
		t.Fatalf("first turn: %s tried %s", out, got)
	}
	remoteVendorsGroup(t, rv, provider.AffinitySession, "office/second/m", "office/third/m")
	if out, got := rv.post(t, s, "s1"); !strings.Contains(out, "from second") || got != "second" {
		t.Fatalf("second turn: %s tried %s", out, got)
	}
	if why := lastWhy(t, s); why != "gone" {
		t.Fatalf("traced as %q, want gone: the remote's other provider kept it as its own", why)
	}
}

func TestRemoteUpstream(t *testing.T) {
	remote := provider.Provider{ID: "office", Preset: provider.RemoteMagpiePreset, Key: "k"}
	plain := provider.Provider{ID: "openrouter", Key: "k"}
	for _, c := range []struct {
		p           provider.Provider
		model, want string
	}{
		{remote, "anthropic/claude-sonnet-4-5", "anthropic"},
		{remote, "openrouter/anthropic/claude-sonnet-4-5", "openrouter"},
		{remote, "group/fast", "group/fast"},
		{remote, "m", ""},
		{plain, "anthropic/claude-sonnet-4-5", ""},
	} {
		if got := remoteUpstream(c.p, c.model); got != c.want {
			t.Errorf("remoteUpstream(%s, %q) = %q, want %q", c.p.ID, c.model, got, c.want)
		}
	}
	a, b := candidate{p: remote, model: "first/m"}, candidate{p: remote, model: "second/m"}
	if a.who() != b.who() || a.vendor() == b.vendor() {
		t.Errorf("who %q %q, vendor %q %q: one key here, two vendors there", a.who(), b.who(), a.vendor(), b.vendor())
	}
	if !a.sameVendor(b.who(), "first/x") || a.sameVendor(b.who(), "second/m") || !a.sameVendor(b.who(), "") {
		t.Error("sameVendor")
	}
}
