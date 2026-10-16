package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

// A plugin's fetch is told the folder the agent works in, as the agent says
// it in its request, and whether it is a git repository, so a plugin that
// tells its vendor the user's environment says the agent's and not
// magpie's own (magpie-community/plugins#86: ZCode's Start Plan said
// magpie's ~/.config/magpie as the working directory, and the model went
// looking there). The headers reach the plugin and never the vendor; a
// request that says no folder has neither.
func TestPluginToldWhereTheAgentWorks(t *testing.T) {
	var mu sync.Mutex
	var got []http.Header
	pid := besideFake(t, "fakeco", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		mu.Lock()
		got = append(got, r.Header.Clone())
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		if r.URL.Path == "/v1/messages" {
			for _, e := range []string{
				`{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"fake-claude","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}`,
				`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}`,
				`{"type":"content_block_stop","index":0}`,
				`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`,
				`{"type":"message_stop"}`,
			} {
				io.WriteString(w, "data: "+e+"\n\n")
			}
			return
		}
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}`+"\n\n")
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	// the agent's own request, sent unstreamed to the model named
	asAgent := func(file, model string) string {
		var b map[string]any
		if err := json.Unmarshal(fixture(t, file), &b); err != nil {
			t.Fatal(err)
		}
		b["model"], b["stream"] = pid+"/"+model, false
		out, _ := json.Marshal(b)
		return string(out)
	}
	repo := filepath.Join(t.TempDir(), "My Repo 中文")
	if err := os.MkdirAll(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	inRepo := filepath.Join(repo, "sub")
	if err := os.MkdirAll(inRepo, 0o755); err != nil {
		t.Fatal(err)
	}
	plain := t.TempDir()
	piAsks := func(dir string) string {
		b, _ := json.Marshal(map[string]any{"model": pid + "/fake-1", "messages": []map[string]any{
			{"role": "system", "content": "You are pi.\n<cwd>\n" + filepath.ToSlash(dir) + "\n</cwd>"},
			{"role": "user", "content": "hi"},
		}})
		return string(b)
	}
	for _, tc := range []struct {
		name, path, body string
		dir, git         string // "" for not told
	}{
		// Claude Code says its folder and that it isn't a git repository;
		// to a model on Anthropic Messages and to one on Chat
		{"claude code, messages", "/v1/messages", asAgent("claude-code-2.1.296.json", "fake-claude"), "/Users/me/work/proj/a&b <c> 中文", "no"},
		{"claude code, chat", "/v1/messages", asAgent("claude-code-2.1.296.json", "fake-1"), "/Users/me/work/proj/a&b <c> 中文", "no"},
		// Codex says its folder only; not a folder on this computer, so
		// whether it is a git repository isn't known
		{"codex", "/v1/responses", asAgent("codex-0.162.0.json", "fake-1"), "/Users/me/work/proj/a&b <c> 中文", ""},
		// a folder here: looked at on disk, the repository above it too
		{"in a repository", "/v1/chat/completions", piAsks(inRepo), filepath.ToSlash(inRepo), "yes"},
		{"not in one", "/v1/chat/completions", piAsks(plain), filepath.ToSlash(plain), "no"},
		{"no folder said", "/v1/chat/completions", `{"model":"` + pid + `/fake-1","messages":[{"role":"user","content":"hi"}]}`, "", ""},
	} {
		mu.Lock()
		got = nil
		mu.Unlock()
		postProto(t, New(), tc.path, tc.body)
		mu.Lock()
		if len(got) != 1 {
			mu.Unlock()
			t.Fatalf("%s: the vendor got %d requests", tc.name, len(got))
		}
		h := got[0]
		mu.Unlock()
		for _, k := range []string{"X-Magpie-Work-Dir", "X-Magpie-Work-Git"} {
			if v := h.Values(k); len(v) > 0 {
				t.Errorf("%s: the vendor got %s: %q", tc.name, k, v)
			}
		}
		dir := h.Get("X-Saw-Work-Dir")
		if dir != "" {
			d, err := url.PathUnescape(dir)
			if err != nil {
				t.Fatalf("%s: %q isn't percent-encoded: %v", tc.name, dir, err)
			}
			dir = d
		}
		if dir != tc.dir || h.Get("X-Saw-Work-Git") != tc.git {
			t.Errorf("%s: the plugin was told %q, git %q; want %q, %q", tc.name, dir, h.Get("X-Saw-Work-Git"), tc.dir, tc.git)
		}
	}
}
