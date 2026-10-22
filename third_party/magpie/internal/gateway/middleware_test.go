package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
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
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
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
	sent := map[string]string{}
	member := func(id string, code int) {
		up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			var b struct {
				System string `json:"system"`
			}
			raw, _ := io.ReadAll(r.Body)
			json.Unmarshal(raw, &b)
			sent[id] = b.System
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
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
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
	if sent["a"] != "asked for a/A/m1/key" || sent["b"] != "asked" {
		t.Fatalf("sent %q", sent)
	}
	if st := middleware.States()[mw]; st.Error != "" || st.Last != "" || !slices.Contains(st.Hooks, "onProvider") {
		t.Fatalf("state %+v", st)
	}
}
