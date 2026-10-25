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

// #2505 (h4rk8s): DSH names its session in x-deepseek-harness-session-id
// and marks a request compacting its context with
// x-deepseek-harness-compact. A call is recorded under that session and as
// a compaction ("compact", the Requests list's Compaction), on this magpie
// and on a remote one it is passed on to; a request without the mark is
// never taken for one by what its prompt says. The vendor is sent neither
// header.
func TestDshMarksSessionAndCompaction(t *testing.T) {
	var mu sync.Mutex
	var vendor []http.Header
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		mu.Lock()
		vendor = append(vendor, r.Header.Clone())
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"chatcmpl-1","object":"chat.completion","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1}}`)
	}))
	t.Cleanup(up.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL}); err != nil {
		t.Fatal(err)
	}
	var arrived []http.Header
	h := New().Handler()
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		arrived = append(arrived, r.Header.Clone())
		mu.Unlock()
		h.ServeHTTP(w, r)
	}))
	t.Cleanup(remote.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "rk", Models: []string{"fake/m1"}, Chat: remote.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	records := func(n int) []usage.Record {
		t.Helper()
		for range 200 {
			var rs []usage.Record
			for _, r := range usage.Load(time.Time{}) {
				if r.Provider == "fake" {
					rs = append(rs, r)
				}
			}
			if len(rs) >= n {
				return rs
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("fewer than %d records", n)
		return nil
	}
	summarize := `"messages":[{"role":"system","content":"Summarize the conversation so far into a compact context."},{"role":"user","content":"hi"}]}`
	for i, tc := range []struct {
		model, compact, kind string
		remote               bool
	}{
		{"fake/m1", "true", "compact", false},
		{"fake/m1", "", "", false}, // a compaction-like prompt, unmarked
		{"office/fake/m1", "1", "compact", true},
		{"office/fake/m1", "", "", true},
	} {
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+tc.model+`",`+summarize))
		req.Header.Set("User-Agent", "dsh/0.1.9")
		req.Header.Set("Authorization", "Bearer "+TokenFor("dsh"))
		req.Header.Set("x-deepseek-harness-session-id", "dsh-session-A")
		if tc.compact != "" {
			req.Header.Set("x-deepseek-harness-compact", tc.compact)
		}
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("%d: %d %s", i, rec.Code, rec.Body.String())
		}
		if tc.remote {
			mu.Lock()
			got := arrived[len(arrived)-1]
			mu.Unlock()
			if got.Get("x-deepseek-harness-session-id") != "dsh-session-A" || got.Get("x-deepseek-harness-compact") != tc.compact {
				t.Errorf("%d: the remote magpie was sent session %q compact %q", i, got.Get("x-deepseek-harness-session-id"), got.Get("x-deepseek-harness-compact"))
			}
		}
		rs := records(i + 1)
		r := rs[len(rs)-1]
		if r.NativeSession != "dsh-session-A" || r.Kind != tc.kind {
			t.Errorf("%d (remote %v): recorded session %q kind %q; want dsh-session-A, %q", i, tc.remote, r.NativeSession, r.Kind, tc.kind)
		}
		if usage.PurposeOf(r.Kind) != map[string]string{"compact": "kind:compact", "": "unmarked"}[tc.kind] {
			t.Errorf("%d: purpose %q", i, usage.PurposeOf(r.Kind))
		}
	}
	mu.Lock()
	defer mu.Unlock()
	for _, v := range vendor {
		for k := range v {
			if strings.HasPrefix(strings.ToLower(k), "x-deepseek-harness-") {
				t.Errorf("the vendor was sent %s", k)
			}
		}
	}
}

// #3298 (tom047784): dsh's own DeepSeek row (llm-deepseek) pointed at magpie
// by hand calls /v1/messages, and dsh's pi-ai routes reach /v1/responses
// too. Each route records dsh's session and compaction as Chat Completions
// does above, and the vendor is sent no x-deepseek-harness-* header, the
// user id llm-deepseek always sends included.
func TestDshMarksOnEveryRoute(t *testing.T) {
	var mu sync.Mutex
	var vendor []http.Header
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		mu.Lock()
		vendor = append(vendor, r.Header.Clone())
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		switch {
		case strings.HasSuffix(r.URL.Path, "/messages"):
			io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}`)
		case strings.HasSuffix(r.URL.Path, "/responses"):
			io.WriteString(w, `{"id":"resp_1","object":"response","status":"completed","model":"m1","output":[{"type":"message","id":"m_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}}`)
		default:
			http.Error(w, "unexpected "+r.URL.Path, 404)
		}
	}))
	t.Cleanup(up.Close)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, p := range []provider.Provider{
		{ID: "ant", Name: "Ant", Key: "k", Models: []string{"m1"}, Anthropic: up.URL + "/v1"},
		{ID: "resp", Name: "Resp", Key: "k", Models: []string{"m1"}, Responses: up.URL + "/v1"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	record := func(id string, n int) usage.Record {
		t.Helper()
		for range 200 {
			var rs []usage.Record
			for _, r := range usage.Load(time.Time{}) {
				if r.Provider == id {
					rs = append(rs, r)
				}
			}
			if len(rs) >= n {
				return rs[len(rs)-1]
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("%s: fewer than %d records", id, n)
		return usage.Record{}
	}
	h := New().Handler()
	seen := map[string]int{}
	for _, tc := range []struct {
		path, prov, body, compact, kind string
	}{
		{"/v1/messages", "ant", `{"model":"ant/m1","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`, "1", "compact"},
		{"/v1/messages", "ant", `{"model":"ant/m1","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}`, "", ""},
		{"/v1/responses", "resp", `{"model":"resp/m1","input":"hi"}`, "1", "compact"},
		{"/v1/responses", "resp", `{"model":"resp/m1","input":"hi"}`, "", ""},
	} {
		req := httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body))
		req.Header.Set("User-Agent", "dsh/0.2.1")
		req.Header.Set("x-api-key", TokenFor("dsh"))
		req.Header.Set("anthropic-version", "2023-06-01")
		req.Header.Set("x-deepseek-harness-user-id", "dsh-user")
		req.Header.Set("x-deepseek-harness-session-id", "dsh-session-B")
		if tc.compact != "" {
			req.Header.Set("x-deepseek-harness-compact", tc.compact)
		}
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		if rec.Code != 200 {
			t.Fatalf("%s: %d %s", tc.path, rec.Code, rec.Body.String())
		}
		seen[tc.prov]++
		r := record(tc.prov, seen[tc.prov])
		if r.NativeSession != "dsh-session-B" || r.Kind != tc.kind {
			t.Errorf("%s compact %q: recorded session %q kind %q; want dsh-session-B, %q", tc.path, tc.compact, r.NativeSession, r.Kind, tc.kind)
		}
	}
	mu.Lock()
	defer mu.Unlock()
	if len(vendor) != 4 {
		t.Fatalf("the vendor got %d requests, want 4", len(vendor))
	}
	for _, v := range vendor {
		for k := range v {
			if strings.HasPrefix(strings.ToLower(k), "x-deepseek-harness-") {
				t.Errorf("the vendor was sent %s", k)
			}
		}
	}
}
