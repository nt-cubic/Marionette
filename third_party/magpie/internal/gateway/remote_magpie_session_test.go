package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// A request one magpie passes on to another (a Remote magpie provider)
// keeps the session its agent named — magpie's X-Magpie-Session and the
// agent's own session_id — so the other magpie records it under that
// session and keeps the conversation apart from another with the same first
// message; they were dropped for an agent that isn't Codex, such as Pi
// (#672). The vendor behind the other magpie is sent neither.
func TestRemoteMagpieKeepsTheSession(t *testing.T) {
	var mu sync.Mutex
	var vendor []http.Header // as the vendor saw each request
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		mu.Lock()
		vendor = append(vendor, r.Header.Clone())
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		if strings.HasSuffix(r.URL.Path, "/responses") {
			io.WriteString(w, `{"id":"resp_1","object":"response","model":"m1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":3,"output_tokens":1}}`)
			return
		}
		io.WriteString(w, `{"id":"chatcmpl-1","object":"chat.completion","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	t.Cleanup(up.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL, Responses: up.URL}); err != nil {
		t.Fatal(err)
	}
	var arrived []http.Header // as the remote magpie was asked
	h := New().Handler()
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		arrived = append(arrived, r.Header.Clone())
		mu.Unlock()
		h.ServeHTTP(w, r)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "rk", Models: []string{"fake/m1"},
		Chat: remote.URL + "/v1", Responses: remote.URL + "/v1", Anthropic: remote.URL}); err != nil {
		t.Fatal(err)
	}

	there := func(n int) usage.Record {
		t.Helper()
		for range 100 {
			var rs []usage.Record
			for _, r := range usage.Load(time.Time{}) {
				if r.Provider == "fake" {
					rs = append(rs, r)
				}
			}
			if len(rs) >= n {
				return rs[n-1]
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("no record %d of the remote's", n)
		return usage.Record{}
	}
	for i, tc := range []struct{ path, body, ua, agent, header, native string }{
		{"/v1/chat/completions", `{"model":"office/fake/m1","messages":[{"role":"user","content":"hi"}]}`, "pi/1.0", "pi", "session_id", "native-A"},
		{"/v1/responses", `{"model":"office/fake/m1","input":[{"role":"user","content":"hi"}],"prompt_cache_key":"cache-A"}`, "pi/1.0", "pi", "session_id", "native-A"},
		{"/v1/chat/completions", `{"model":"office/fake/m1","messages":[{"role":"user","content":"hi"}]}`, "CLI/2.161.0 WorkBuddy/2.161.0", "workbuddy", "X-Conversation-ID", "workbuddy-A"},
		{"/v1/responses", `{"model":"office/fake/m1","input":[{"role":"user","content":"hi"}]}`, "CLI/2.161.0 WorkBuddy/2.161.0", "workbuddy", "X-Conversation-ID", "workbuddy-A"},
	} {
		req := httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body))
		req.Header.Set("User-Agent", tc.ua)
		req.Header.Set("Authorization", "Bearer "+TokenFor(tc.agent))
		req.Header.Set(SessionHeader, "trace-A")
		req.Header.Set(tc.header, tc.native)
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, req)
		if rec.Code != 200 || !strings.Contains(rec.Body.String(), "hello") {
			t.Fatalf("%s: %d %s", tc.path, rec.Code, rec.Body.String())
		}
		mu.Lock()
		got := arrived[len(arrived)-1]
		mu.Unlock()
		if got.Get(SessionHeader) != "trace-A" || got.Get(tc.header) != tc.native {
			t.Errorf("%s: the remote magpie was sent %s=%q %s=%q; want trace-A, %s", tc.path, SessionHeader, got.Get(SessionHeader), tc.header, got.Get(tc.header), tc.native)
		}
		if r := there(i + 1); r.Session != "trace-A" || r.NativeSession != tc.native || r.Agent != tc.agent {
			t.Errorf("%s: the remote recorded agent %q session %q native %q; want %s, trace-A, %s", tc.path, r.Agent, r.Session, r.NativeSession, tc.agent, tc.native)
		}
	}
	mu.Lock()
	defer mu.Unlock()
	for _, v := range vendor {
		for k := range v {
			if lk := strings.ToLower(k); strings.HasPrefix(lk, "x-magpie-") || lk == "session_id" || lk == "x-conversation-id" {
				t.Errorf("the vendor was sent %s", k)
			}
		}
	}
}
