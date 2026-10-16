package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// codexShaped is a Responses request as codex-cli sends one: developer
// messages with its permissions (a writable root under the user's home)
// and the user's own, then the prompt. What Privacy masks in them is what
// made magpie wrap the reply to put the values back (#2402); a plain
// {"input": "<prompt>"} has nothing to mask, and its reply went unwrapped.
const codexShaped = `{"model":"fixture/gpt-test","stream":true,"store":false,"instructions":"You are Codex.",
 "input":[
  {"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>Filesystem sandboxing: workspace-write. Writable roots: /Users/moon/work. Git author moon.shard@proton.me</permissions instructions>"}]},
  {"type":"message","role":"developer","content":[{"type":"input_text","text":"<collaboration_mode>default</collaboration_mode>"}]},
  {"type":"message","role":"user","content":[{"type":"input_text","text":"think hard, then say hello"}]}],
 "reasoning":{"effort":"xhigh"},"tool_choice":"auto","parallel_tool_calls":true}`

// A Codex request through a Responses provider whose vendor sent
// response.created and then reasoned in silence: magpie wrote a
// ": keepalive" every keepaliveEvery, but with something in the request
// masked the reply went through the writer that puts the values back, and
// it held each comment for the event after it, as a part of that event. The
// six of 90s came together with the first words, and Cloudflare in front of
// magpie had hung up at 100s with a 524 (#2402, moonshardx). A JS
// middleware's onEvent wraps the reply the same way. The vendor here says
// nothing more until the agent has read a keepalive, so a held one waits
// out its 3s and fails.
func TestQuietCodexStreamKeptAliveThroughReplyWriters(t *testing.T) {
	for _, tc := range []struct {
		name              string
		masks, middleware bool
	}{{"masked", true, false}, {"a middleware", false, true}} {
		t.Run(tc.name, func(t *testing.T) {
			fresh(t)
			quietFast(t)
			t.Cleanup(plugin.Settle)
			heard := make(chan struct{})
			var timedOut atomic.Bool
			var sent []byte
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				sent, _ = io.ReadAll(r.Body)
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, quietCreated)
				w.(http.Flusher).Flush()
				select {
				case <-heard:
				case <-time.After(3 * time.Second):
					timedOut.Store(true)
				case <-r.Context().Done():
					return
				}
				io.WriteString(w, quietDelta("hello")+quietDone)
			}))
			t.Cleanup(up.Close)
			if err := provider.Save(provider.Provider{ID: "fixture", Name: "Fixture", Key: "k", Models: []string{"gpt-test"}, Responses: up.URL + "/v1"}); err != nil {
				t.Fatal(err)
			}
			if err := settings.Save(settings.Settings{Redact: tc.masks, RedactPersonal: tc.masks}); err != nil {
				t.Fatal(err)
			}
			if tc.middleware {
				mw := filepath.Join(t.TempDir(), "upper.middleware.js")
				os.WriteFile(mw, []byte(`export function onEvent(ev) { if (ev.delta) ev.delta = ev.delta.toUpperCase(); return ev; }`), 0o644)
				b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
				os.MkdirAll(settings.Dir(), 0o755)
				os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
				middleware.Reload()
				t.Cleanup(middleware.Reload)
			}

			gw := httptest.NewServer(New().Handler())
			t.Cleanup(gw.Close)
			resp, err := http.Post(gw.URL+"/v1/responses", "application/json", strings.NewReader(codexShaped))
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()
			body := readQuiet(resp.Body, heard, "")

			if tc.masks == strings.Contains(string(sent), "moon.shard@proton.me") {
				t.Fatalf("masked %v, sent %s", tc.masks, sent)
			}
			alive, words := strings.Index(body, ": keepalive"), strings.Index(body, "response.output_text.delta")
			if timedOut.Load() || resp.StatusCode != 200 || alive < 0 || words < alive {
				t.Fatalf("the agent heard no keepalive while the vendor was quiet (held %v): %d %q", timedOut.Load(), resp.StatusCode, body)
			}
			hello := "hello"
			if tc.middleware {
				hello = "HELLO"
			}
			if !strings.Contains(body[alive:], hello) || !strings.Contains(body, "response.completed") {
				t.Fatalf("the reply after the keepalives: %q", body)
			}
		})
	}
}
