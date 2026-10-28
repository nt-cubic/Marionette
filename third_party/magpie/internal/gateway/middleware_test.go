package gateway

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/middleware"
	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A gateway middleware sees an agent's request before routing and its
// reply's events in the agent's own API, and can turn a request away.
func TestMiddlewareThroughGateway(t *testing.T) {
	const key = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = io.ReadAll(r.Body)
		chunk := func(s string) string {
			b, _ := json.Marshal(map[string]any{"id": "c1", "model": "m1", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": s}}}})
			return "data: " + string(b)
		}
		w.Header().Set("Content-Type", "text/event-stream")
		// the reply says the key back, which redaction gave it as a placeholder
		var sent struct {
			Messages []struct{ Content any } `json:"messages"`
		}
		json.Unmarshal(got, &sent)
		last, _ := json.Marshal(sent.Messages[len(sent.Messages)-1].Content)
		io.WriteString(w, sse(chunk("hello "), chunk(string(last)), `data: {"id":"c1","model":"m1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}`, "data: [DONE]"))
	}))
	defer up.Close()
	fresh(t) // nothing another test left on "fake" (#3273)
	// the plugins' list written below reads as changed, and what that
	// tells the plugins' hooks ends with the test
	t.Cleanup(plugin.Settle)
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Chat: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	mw := filepath.Join(t.TempDir(), "alias.middleware.js")
	os.WriteFile(mw, []byte(`
export function onRequest(body, ctx) {
  if (body.model === "blocked") return ctx.reject(402, "no budget left");
  ctx.state.asked = body.model;
  if (body.model === "fast") body.model = "fake/m1";
  return body;
}
export const events = ["content_block_delta"];
export function onEvent(ev, ctx) {
  if (ev.delta && ev.delta.type === "text_delta") ev.delta.text = ev.delta.text.toUpperCase() + "[" + ctx.protocol + "]";
  return ev;
}`), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
	middleware.Reload()
	defer middleware.Reload()

	code, body := post(t, "/v1/messages", `{"model":"fast","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"key `+key+`"}]}`)
	if code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	if strings.Contains(string(got), key) {
		t.Fatalf("the middleware's body went out unredacted: %s", got)
	}
	var text strings.Builder
	for _, e := range events(body) {
		if d, ok := e["delta"].(map[string]any); ok {
			s, _ := d["text"].(string)
			text.WriteString(s)
		}
	}
	// the middleware saw the key as the agent will, not the placeholder
	if want := "HELLO [anthropic]" + strings.ToUpper(`"key `+key+`"`) + "[anthropic]"; text.String() != want {
		t.Fatalf("text %q, want %q", text.String(), want)
	}

	code, body = post(t, "/v1/messages", `{"model":"blocked","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 402 || !strings.Contains(body, "no budget left") || !strings.Contains(body, `"type":"error"`) {
		t.Fatalf("reject: %d %s", code, body)
	}
	st := middleware.States()[mw]
	if st.Failures != 0 || st.Calls < 4 {
		t.Fatalf("state %+v", st)
	}
}

// onProvider changes the body of one try only: a group's first member is
// sent what the middleware made of it for that member, and the member
// tried next gets the body onRequest left, not the first try's.
func TestMiddlewareOnProviderPerTry(t *testing.T) {
	// no rest, affinity or turn another test left on "a" sends the group
	// past it (#3273: TestReplyNamesMember's 529 left "a" resting)
	fresh(t)
	var mu sync.Mutex
	sent := map[string]string{}
	member := func(id string, code int) {
		up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var b struct {
				System string `json:"system"`
			}
			raw, _ := io.ReadAll(r.Body)
			json.Unmarshal(raw, &b)
			mu.Lock()
			sent[id] = b.System
			mu.Unlock()
			if code != 200 {
				http.Error(w, `{"type":"error","error":{"type":"api_error","message":"down"}}`, code)
				return
			}
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"id":"m","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`)
		}))
		t.Cleanup(up.Close)
		if err := provider.Save(provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Models: []string{"m1"}, Anthropic: up.URL}); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(plugin.Settle)
	member("a", 500)
	member("b", 200)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m1", "b/m1"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	mw := filepath.Join(t.TempDir(), "per-try.middleware.js")
	os.WriteFile(mw, []byte(`
export function onRequest(body) { body.system = "asked"; return body; }
export function onProvider(body, ctx) {
  const p = ctx.provider;
  if (p.id === "a") body.system += " for " + [p.id, p.name, p.model, p.account || "key"].join("/");
  return body;
}`), 0o644)
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644)
	middleware.Reload()
	defer middleware.Reload()

	code, body := post(t, "/v1/messages", `{"model":"group/g","max_tokens":10,"system":"agent's","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	mu.Lock()
	defer mu.Unlock()
	if sent["a"] != "asked for a/A/m1/key" || sent["b"] != "asked" {
		t.Fatalf("sent %q", sent)
	}
	if st := middleware.States()[mw]; st.Error != "" || st.Last != "" || !slices.Contains(st.Hooks, "onProvider") {
		t.Fatalf("state %+v", st)
	}
}

// onMiddleware puts a middleware of src on for t, alone in plugins.json.
func onMiddleware(t *testing.T, src string) string {
	t.Helper()
	// the plugins' list written below reads as changed, and what that
	// tells the plugins' hooks ends with the test
	t.Cleanup(plugin.Settle)
	mw := filepath.Join(t.TempDir(), "mark.middleware.js")
	if err := os.WriteFile(mw, []byte(src), 0o644); err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(plugin.List{Plugins: []plugin.Entry{{Spec: mw}}})
	os.MkdirAll(settings.Dir(), 0o755)
	if err := os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
	middleware.Reload()
	t.Cleanup(middleware.Reload)
	return mw
}

// A request's images, each described by the model that sees, are magpie's
// own asks and go without the agent's middleware. Up to four are asked at
// once, and ran the request's onProvider on its one runtime from as many
// goroutines: the gateway died of "concurrent map read and map write" in
// moejs (#3695, aiirobyte). The agent's own try still goes through it.
func TestImagesDescribedWithoutTheAgentsMiddleware(t *testing.T) {
	s, u := eyed(t)
	mw := onMiddleware(t, `
export function onProvider(body, ctx) {
  body.messages.push({role: "user", content: "onProvider for " + ctx.provider.model});
  return body;
}`)
	var parts []string
	for i := range visionParallel {
		img := base64.StdEncoding.EncodeToString(fmt.Appendf(nil, "screenshot %d", i))
		parts = append(parts, `{"type":"image_url","image_url":{"url":"data:image/png;base64,`+img+`"}}`)
	}
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(
		`{"model":"probe/text","messages":[{"role":"user","content":[{"type":"text","text":"read"},`+strings.Join(parts, ",")+`]}]}`)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	eye := u.sent("eye")
	if len(eye) != visionParallel {
		t.Fatalf("%d images described, want %d", len(eye), visionParallel)
	}
	for _, b := range eye {
		if strings.Contains(b, "onProvider for") {
			t.Errorf("a description was asked through the agent's middleware: %s", b)
		}
	}
	if text := u.sent("text"); len(text) != 1 || !strings.Contains(text[0], "onProvider for text") {
		t.Fatalf("the agent's model was sent %q", text)
	}
	if st := middleware.States()[mw]; st.Calls != 1 || st.Failures != 0 {
		t.Fatalf("state %+v, want onProvider called once, on the agent's try", st)
	}
}

// The same for a web search magpie makes for a model that can't search:
// it is asked beside the reply the agent is streamed through the
// middleware's onEvent, so one search is enough to share the runtime, and
// a round's searches beside each other (#3695). A hook that throws lets a
// search through unmarked, so none may.
func TestSearchesWithoutTheAgentsMiddleware(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	var searched, asked []string
	search := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		switch r.URL.Path {
		case "/v1/models":
			io.WriteString(w, `{"data":[{"id":"claude-opus-5-5"},{"id":"claude-haiku-4-5"}]}`)
		case "/v1/messages":
			mu.Lock()
			searched = append(searched, string(b))
			mu.Unlock()
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"id":"s","type":"message","role":"assistant","model":"claude-haiku-4-5","content":[
				{"type":"server_tool_use","id":"srv","name":"web_search","input":{"query":"go"}},
				{"type":"web_search_tool_result","tool_use_id":"srv","content":[{"type":"web_search_result","title":"Go downloads","url":"https://go.dev/dl/"}]},
				{"type":"text","text":"Found it."}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":5}}`)
		default:
			http.NotFound(w, r)
		}
	}))
	defer search.Close()
	model := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		asked = append(asked, string(b))
		n := len(asked)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		if n == 1 {
			// two searches in one round
			io.WriteString(w, sse(`data: {"id":"c","choices":[{"index":0,"delta":{"role":"assistant","content":"Let me check."}}]}`,
				`data: {"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"latest go\"}"}}]}}]}`,
				`data: {"id":"c","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_2","type":"function","function":{"name":"web_search","arguments":"{\"query\":\"latest rust\"}"}}]}}]}`,
				`data: {"id":"c","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":30,"completion_tokens":7}}`,
				`data: [DONE]`))
			return
		}
		io.WriteString(w, sse(`data: {"id":"d","choices":[{"index":0,"delta":{"role":"assistant","content":"Both are out."}}]}`,
			`data: {"id":"d","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}`,
			`data: [DONE]`))
	}))
	defer model.Close()
	hosts := searchHosts[provider.Anthropic]
	searchHosts[provider.Anthropic] = append(hosts, provider.HostOf(search.URL))
	defer func() { searchHosts[provider.Anthropic] = hosts }()
	for _, p := range []provider.Provider{
		{ID: "srch", Name: "Search", Key: "k", Anthropic: search.URL},
		{ID: "deep", Name: "Deep", Key: "k", Chat: model.URL, Models: []string{"deep-chat"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	p, _ := provider.Find("srch")
	if _, err := p.Fetch(context.Background()); err != nil {
		t.Fatal(err)
	}
	if sp, m, ok := searcher(); !ok || sp.ID != "srch" || m != "claude-haiku-4-5" {
		t.Fatalf("searcher = %s %s %v", sp.ID, m, ok)
	}
	mw := onMiddleware(t, `
export function onProvider(body, ctx) {
  body.system = "onProvider for " + ctx.provider.model;
  return body;
}
export const events = ["content_block_delta"];
export function onEvent(ev) { return ev; }`)

	code, out := post(t, "/v1/messages", `{"model":"deep/deep-chat","max_tokens":1000,"stream":true,"messages":[{"role":"user","content":"What are the latest Go and Rust?"}],
		"tools":[{"type":"web_search_20250305","name":"web_search","max_uses":8}]}`)
	if code != 200 || !strings.Contains(out, "Both are out.") {
		t.Fatalf("%d %s", code, out)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(searched) != 2 {
		t.Fatalf("searched %d times, want 2", len(searched))
	}
	for _, b := range searched {
		if strings.Contains(b, "onProvider for") {
			t.Errorf("a search was asked through the agent's middleware: %s", b)
		}
	}
	if len(asked) != 2 || !strings.Contains(asked[0], "onProvider for deep-chat") {
		t.Fatalf("the agent's model was asked %d times, first with %s", len(asked), asked[0])
	}
	if st := middleware.States()[mw]; st.Failures != 0 {
		t.Fatalf("state %+v, want no hook failed", st)
	}
}
