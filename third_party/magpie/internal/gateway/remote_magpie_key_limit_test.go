package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
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
