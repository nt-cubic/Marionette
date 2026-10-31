package gateway

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

// A plugin's model listed by an id of the user's own (zeewell on X:
// Cursor's deepseek-v41-flash-1017, Devin's, WorkBuddy's) is listed and
// asked for by that id, on each of the plugin's APIs, and the plugin's
// fetch is still asked for the model by the id its own list gives it —
// in the body, the URL and the model header the plugin reads.
func TestLocalModelIDAtAPlugin(t *testing.T) {
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)

	type seen struct{ path, model, body string }
	var mu sync.Mutex
	var got []seen
	up := testenv.FakeVendor(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = append(got, seen{r.URL.RequestURI(), r.Header.Get("X-Plugin-Model"), string(b)})
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		switch {
		case r.URL.Path == "/v1/chat/completions":
			fmt.Fprint(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"hi from chat"},"finish_reason":null}]}`+"\n\n")
			fmt.Fprint(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
		case r.URL.Path == "/v1/messages":
			for _, e := range []string{
				`{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"fake-claude","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}`,
				`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi from claude"}}`,
				`{"type":"content_block_stop","index":0}`,
				`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}`,
				`{"type":"message_stop"}`,
			} {
				fmt.Fprintf(w, "data: %s\n\n", e)
			}
		case strings.HasPrefix(r.URL.Path, "/v1/models/"):
			fmt.Fprint(w, `data: {"candidates":[{"content":{"role":"model","parts":[{"text":"hi from gemini"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2}}`+"\n\n")
		default:
			w.WriteHeader(404)
		}
	}))
	defer up.Close()

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.APIKey(ctx, "fakeco", 0, nil, "k1", plugin.NewAccount); err != nil {
		t.Fatal(err)
	}

	listedAs := map[string]string{"fake-1": "my-flash", "fake-claude": "my-claude", "fake-gemini": "my-gemini"}
	for own, id := range listedAs {
		if _, changed, err := provider.SetModelID("fakeco/"+own, id); err != nil || !changed {
			t.Fatalf("SetModelID(fakeco/%s, %s) = %v, %v", own, id, changed, err)
		}
	}
	p, err := provider.Find("fakeco")
	if err != nil || !p.IsPlugin() || !p.TakesModelIDs() {
		t.Fatalf("Find = %+v, %v", p, err)
	}
	for model, want := range map[string]provider.Protocol{"my-flash": provider.Chat, "my-claude": provider.Anthropic, "my-gemini": provider.CodeAssist} {
		if apis := p.APIs(model); len(apis) != 1 || apis[0] != want {
			t.Fatalf("APIs(%s) = %v", model, apis)
		}
	}

	s := New()
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/v1/models", nil))
	var l struct{ Data []struct{ ID string } }
	if err := json.Unmarshal(rec.Body.Bytes(), &l); err != nil {
		t.Fatal(rec.Body.String())
	}
	var ids []string
	for _, m := range l.Data {
		ids = append(ids, m.ID)
	}
	for own, id := range listedAs {
		if !slices.Contains(ids, "fakeco/"+id) || slices.Contains(ids, "fakeco/"+own) {
			t.Errorf("/v1/models lists fakeco's %s as: %v", own, ids)
		}
	}

	for _, c := range []struct{ model, own, text, path string }{
		{"fakeco/my-flash", "fake-1", "hi from chat", "/v1/chat/completions"},
		{"fakeco/my-claude", "fake-claude", "hi from claude", "/v1/messages"},
		{"fakeco/my-gemini", "fake-gemini", "hi from gemini", "/v1/models/fake-gemini:streamGenerateContent?alt=sse"},
	} {
		code, body := postAs(t, s, "", `{"model":"`+c.model+`","messages":[{"role":"user","content":"hello"}]}`)
		if code != 200 || !strings.Contains(body, c.text) {
			t.Fatalf("%s: %d %s", c.model, code, body)
		}
		mu.Lock()
		last := got[len(got)-1]
		mu.Unlock()
		if last.path != c.path || last.model != c.own || strings.Contains(last.body, "my-") {
			t.Fatalf("%s reached the plugin's vendor as %+v", c.model, last)
		}
		if r := lastUsage(t); r.Model != strings.TrimPrefix(c.model, "fakeco/") {
			t.Errorf("%s was counted as %q", c.model, r.Model)
		}
	}
}

// An Antigravity family the user lists by an id of theirs goes out as the
// variant of the vendor's id the effort picks, not as the family's bare id
// a wire name gives it.
func TestLocalModelIDAtAnAntigravityAccount(t *testing.T) {
	fresh(t)
	if err := os.MkdirAll(filepath.Dir(catalog.LivePath("antigravity")), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.LivePath("antigravity"), []byte(`{"models":[{"id":"flash-9-low"},{"id":"flash-9-high"}]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	s := settings.Load()
	s.ModelIDs = map[string]string{"antigravity/my-flash": "flash-9"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	p := provider.Provider{ID: "antigravity", Account: &provider.Account{Agent: "antigravity", User: "u"}}
	r, err := parseChat([]byte(`{"model":"antigravity/my-flash","stream":true,"reasoning_effort":"low","messages":[{"role":"user","content":"hi"}]}`))
	if err != nil {
		t.Fatal(err)
	}
	var env struct{ Model string }
	if err := json.Unmarshal(codeAssistBody(p, r, "my-flash", settings.Load().Wires()), &env); err != nil {
		t.Fatal(err)
	}
	if env.Model != "flash-9-low" {
		t.Fatalf("my-flash at low went out as %q, want flash-9-low", env.Model)
	}
}

// A Claude account's model the user lists by an id of theirs is run in
// Claude Code by Anthropic's id: no wire name reaches the run.
func TestLocalModelIDAtAClaudeAccount(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("a shell script stands in for Claude Code")
	}
	fresh(t)
	s := settings.Load()
	s.ModelIDs = map[string]string{"claude/my-sonnet": "claude-sonnet-5"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	argv := filepath.Join(dir, "argv")
	testenv.Program(t, filepath.Join(dir, "claude"), `#!/bin/sh
echo "$@" > '`+argv+`'
read -r line
echo '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m","model":"claude-sonnet-5","usage":{"input_tokens":1}}}}'
echo '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}'
echo '{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}}'
echo '{"type":"stream_event","event":{"type":"message_stop"}}'
`)
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))
	srv := New()
	t.Cleanup(srv.subscription.abortAll)
	p := provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude", User: "u"}}
	if own := p.OwnModelID("my-sonnet"); own != "claude-sonnet-5" {
		t.Fatalf("OwnModelID(my-sonnet) = %q", own)
	}
	body := `{"model":"claude/my-sonnet","max_tokens":100,"stream":true,"messages":[{"role":"user","content":"hi"}]}`
	rec := httptest.NewRecorder()
	var u Usage
	srv.serveClaudeSubscription(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)), provider.Anthropic, p, "my-sonnet", []byte(body), &u)
	b, err := os.ReadFile(argv)
	if err != nil {
		t.Fatalf("Claude Code never ran: %v; %s", err, rec.Body.String())
	}
	if args := string(b); !strings.Contains(args, "claude-sonnet-5") || strings.Contains(args, "my-sonnet") {
		t.Fatalf("Claude Code ran as: %s", args)
	}
}
