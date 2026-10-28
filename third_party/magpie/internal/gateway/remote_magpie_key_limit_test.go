package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #2077: a magpie that uses another computer's (a Remote magpie provider)
// was told by it that Claude Code was out of its session limit, and
// rested its key to that magpie: a request for one of the other magpie's
// Qoder models was refused here with Claude Code's 429, never asked
// there. A refusal from the other magpie for one of its models is that
// model's; only the key's own limit there rests the key.
func TestRemoteMagpieRestsTheModel(t *testing.T) {
	spent := `{"type":"error","error":{"type":"rate_limit_error","message":"You've hit your session limit · resets 8:10pm (America/Los_Angeles)"}}`
	cc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusTooManyRequests)
		io.WriteString(w, spent)
	}))
	t.Cleanup(cc.Close)
	var asked atomic.Int32
	q := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		asked.Add(1)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","object":"chat.completion","model":"fast","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	t.Cleanup(q.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, p := range []provider.Provider{
		{ID: "cc", Name: "Claude Code", Key: "k1", Models: []string{"claude-opus-5-5"}, Anthropic: cc.URL},
		{ID: "qd", Name: "Qoder", Key: "k2", Models: []string{"fast"}, Chat: q.URL},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	remote := httptest.NewServer(New().Handler())
	t.Cleanup(remote.Close)
	id, err := provider.Add(provider.Provider{ID: "office", Name: "Remote magpie", Preset: provider.RemoteMagpiePreset, Chat: strings.TrimPrefix(remote.URL, "http://")})
	if err != nil {
		t.Fatal(err)
	}
	office, _ := provider.Find(id)
	if _, err := office.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}
	send := func(model string) *httptest.ResponseRecorder {
		t.Helper()
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`))
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, req)
		return rec
	}
	if rec := send(id + "/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests {
		t.Fatalf("Claude Code out of its session limit there: %d %s; want its 429", rec.Code, rec.Body.String())
	}
	rec := send(id + "/qd/fast")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "hello") {
		t.Fatalf("Qoder there, after Claude Code's 429: %d %s; want Qoder's answer", rec.Code, rec.Body.String())
	}
	if asked.Load() != 1 {
		t.Errorf("Qoder was asked %d times; want once", asked.Load())
	}
	// Claude Code itself still rests: refused again (a rest of a length
	// magpie guessed is asked once more after the first refusal, zoecats
	// on Discord), it is refused here without the other magpie being asked
	if rec := send(id + "/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests {
		t.Fatalf("Claude Code asked again: %d %s; want its 429", rec.Code, rec.Body.String())
	}
	if rec := send(id + "/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests || !strings.Contains(rec.Body.String(), "not asked again") {
		t.Errorf("Claude Code asked again while it rests: %d %s; want its 429 held here", rec.Code, rec.Body.String())
	}
}

// The key's own limit on the other magpie (keyLimited's 429, marked with
// remoteKeyLimitHeader) is the key's whatever model it was asked for: the
// key rests, every model with it.
func TestRemoteMagpieKeyLimitRestsTheKey(t *testing.T) {
	c := candidate{p: provider.Provider{ID: "office", Preset: provider.RemoteMagpiePreset, Key: "k"}, model: "cc/claude-opus-5-5", rest: "office"}
	h := http.Header{}
	if !remoteModelRest(c, failQuota, h) {
		t.Error("a quota refusal from the other magpie rests the key, not the model")
	}
	h.Set(remoteKeyLimitHeader, "2026-10-15T00:00:00Z")
	if remoteModelRest(c, failQuota, h) || remoteModelRest(c, failRate, h) {
		t.Error("the key's own limit there rests only the model, not the key")
	}
	c.p.Preset = ""
	if remoteModelRest(c, failQuota, http.Header{}) {
		t.Error("a provider that isn't a remote magpie rests the model on a quota refusal")
	}
}

// lagrangepoint1813 on Discord: another computer's magpie added as an
// OpenAI-compatible provider of its own name, not the Remote magpie
// preset, said its Claude Code was out of its session limit, and every
// model of it was refused here with that 429 for the five hours it
// named, whichever one Pi was switched to. A reply that came from a
// magpie (it names the provider that answered, providerHeader) is that
// magpie's model's, whatever the provider here was added as.
func TestAnotherMagpieAddedByHandRestsTheModel(t *testing.T) {
	t.Run("asked there", func(t *testing.T) { anotherMagpieAddedByHand(t, false) })
	t.Run("held there", func(t *testing.T) { anotherMagpieAddedByHand(t, true) })
}

func anotherMagpieAddedByHand(t *testing.T, held bool) {
	clineRestsCleared(t)
	spent := `{"type":"error","error":{"type":"rate_limit_error","message":"You've hit your session limit · resets 8:10pm (America/Los_Angeles)"}}`
	cc := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusTooManyRequests)
		io.WriteString(w, spent)
	}))
	t.Cleanup(cc.Close)
	var asked atomic.Int32
	q := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		asked.Add(1)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"c1","object":"chat.completion","model":"fast","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	t.Cleanup(q.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, p := range []provider.Provider{
		{ID: "cc", Name: "Claude Code", Key: "k1", Models: []string{"claude-opus-5-5"}, Anthropic: cc.URL},
		{ID: "qd", Name: "Qoder", Key: "k2", Models: []string{"fast"}, Chat: q.URL},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	remote := httptest.NewServer(New().Handler())
	t.Cleanup(remote.Close)
	// as it is added by hand: an OpenAI-compatible endpoint, a key, the
	// other magpie's model ids
	if err := provider.Save(provider.Provider{ID: "rm", Name: "Remote Magpie", Key: "k3", Chat: remote.URL + "/v1",
		Models: []string{"cc/claude-opus-5-5", "qd/fast"}}); err != nil {
		t.Fatal(err)
	}
	send := func(model string) *httptest.ResponseRecorder {
		t.Helper()
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+model+`","messages":[{"role":"user","content":"hi"}]}`))
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, req)
		return rec
	}
	if held {
		// the other magpie already holds Claude Code, refused twice
		// there, and answers it without asking: its answer names the
		// provider all the same
		for range 2 {
			send("cc/claude-opus-5-5")
		}
		if rec := send("cc/claude-opus-5-5"); !strings.Contains(rec.Body.String(), "not asked again") {
			t.Fatalf("the other magpie asked Claude Code again: %d %s; want it held there", rec.Code, rec.Body.String())
		}
	}
	if rec := send("rm/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests {
		t.Fatalf("Claude Code out of its session limit there: %d %s; want its 429", rec.Code, rec.Body.String())
	}
	rec := send("rm/qd/fast")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "hello") {
		t.Fatalf("Qoder there, after Claude Code's 429: %d %s; want Qoder's answer", rec.Code, rec.Body.String())
	}
	if asked.Load() != 1 {
		t.Errorf("Qoder was asked %d times; want once", asked.Load())
	}
	if rec := send("rm/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests {
		t.Fatalf("Claude Code asked again: %d %s; want its 429", rec.Code, rec.Body.String())
	}
	if rec := send("rm/cc/claude-opus-5-5"); rec.Code != http.StatusTooManyRequests || !strings.Contains(rec.Body.String(), "not asked again") {
		t.Errorf("Claude Code asked again while it rests: %d %s; want its 429 held here", rec.Code, rec.Body.String())
	}
	// held here now, Claude Code holds nothing else of that magpie's
	rec = send("rm/qd/fast")
	if rec.Code != http.StatusOK || !strings.Contains(rec.Body.String(), "hello") {
		t.Fatalf("Qoder there, while Claude Code is held here: %d %s; want Qoder's answer", rec.Code, rec.Body.String())
	}
	if asked.Load() != 2 {
		t.Errorf("Qoder was asked %d times; want twice", asked.Load())
	}
}

// The marker comes by wire: the other magpie's keyLimited answers a 429
// with remoteKeyLimitHeader, and what reaches restAfterMarked here is what
// the relay kept of the upstream reply. The key's own limit there rests
// the whole remote, not the model it was asked for — the opposite of that
// model's own rate limit, which rests it alone
// (TestRemoteMagpieQuotaRestsTheModel). So asked again, the remote's
// models are behind the next member, neither of them asked.
func TestRemoteMagpieKeyLimitRestsTheRemote(t *testing.T) {
	fresh(t)
	forgetSinks(t)
	var mu sync.Mutex
	var asked []string
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string `json:"model"`
		}
		json.NewDecoder(r.Body).Decode(&req)
		mu.Lock()
		asked = append(asked, req.Model)
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if req.Model == "glm/glm-5" {
			// what keyLimited answers with: its 429 names when the key
			// resets, and marks itself the key's own
			w.Header().Set(remoteKeyLimitHeader, "2026-10-19T00:00:00Z")
			w.Header().Set("Retry-After", "60")
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"gateway key over its request budget","type":"rate_limit_error"}}`)
			return
		}
		if req.Model == "kimi/kimi-k3" {
			// that model's own rate limit there, no marker: rests alone
			w.WriteHeader(http.StatusTooManyRequests)
			io.WriteString(w, `{"error":{"message":"rate limit exceeded, slow down"}}`)
			return
		}
		io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"from `+req.Model+`"},"finish_reason":"stop"}]}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"glm/glm-5", "kimi/kimi-k3"}}); err != nil {
		t.Fatal(err)
	}
	other := &keyed{}
	serveOn(t, "other", "ko", []string{"m"}, other)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"office/glm/glm-5", "office/kimi/kimi-k3", "other/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	// neither of the remote's models answers: the turn goes to the next
	// member
	if code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(body, "from ko") {
		t.Fatalf("the remote's key limit and its model's rate limit: %d %s", code, body)
	}
	// the key's own limit there rests the remote whole; kimi's own rate
	// limit rests kimi alone
	if _, ok := restOf("office"); !ok {
		t.Errorf("the key's own limit there didn't rest the remote whole: %v", rests())
	}
	if r, ok := restOf("office/glm/glm-5"); ok {
		t.Errorf("the key's own limit rested glm alone: %+v %v", r, rests())
	}
	if _, ok := restOf("office/kimi/kimi-k3"); !ok {
		t.Errorf("kimi's own rate limit there didn't rest kimi alone: %v", rests())
	}
	// asked again, the remote sits out whole: neither model is
	mu.Lock()
	asked = nil
	mu.Unlock()
	if code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`); code != 200 || !strings.Contains(body, "from ko") || len(asked) != 0 {
		t.Fatalf("the remote's models were asked %v after its key limit: %d %s", asked, code, body)
	}
}

// The other magpie's key-limit marker is read here, not passed on: a
// magpie that asks this one would read it as this magpie's own key limit
// and rest all of this one, every provider behind it, for one key there
// (#2791). The agent gets the 429 without the marker or magpie's own
// note of it.
func TestRemoteMagpieKeyLimitMarkerStaysHere(t *testing.T) {
	fresh(t)
	forgetSinks(t)
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set(remoteKeyLimitHeader, "2026-10-19T00:00:00Z")
		w.WriteHeader(http.StatusTooManyRequests)
		io.WriteString(w, `{"error":{"message":"gateway key over its request budget","type":"rate_limit_error"}}`)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "k",
		Chat: remote.URL + "/v1", Models: []string{"glm/glm-5"}}); err != nil {
		t.Fatal(err)
	}
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions",
		strings.NewReader(`{"model":"office/glm/glm-5","messages":[{"role":"user","content":"hi"}]}`)))
	if rec.Code != http.StatusTooManyRequests {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	for _, h := range []string{remoteKeyLimitHeader, keyLimitThereHeader} {
		if v := rec.Header().Get(h); v != "" {
			t.Errorf("the agent's reply carries %s: %q", h, v)
		}
	}
}
