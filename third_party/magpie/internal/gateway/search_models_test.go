package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Command Code's Provider API searches the web by itself for some of its
// models only (#1590, M3chD09: 12 of 87, its GPT, Grok and Muse models on
// Responses, its Claude ones on Messages). Those are sent the agent's own
// web search tool; the rest keep magpie's searcher, and so does every
// model when the provider is sent Command Code's zero-data-retention
// header, under which it refuses every built-in tool.
func TestCommandCodeSearchesForSomeModels(t *testing.T) {
	srv, seen := relayOfEvery(t)
	fresh(t)
	save := func(h map[string]string) {
		t.Helper()
		if err := provider.Save(provider.Provider{ID: "commandcode", Name: "Command Code", Key: "k", Preset: "commandcode",
			Chat: srv.URL + "/v1", Responses: srv.URL + "/v1", Anthropic: srv.URL, Headers: h,
			Models: []string{"gpt-6-luna", "xai/grok-4.7", "deepseek/deepseek-v4-pro", "claude-sonnet-5-5", "claude-opus-5-5"}}); err != nil {
			t.Fatal(err)
		}
	}
	codex := func(model string) string {
		return `{"model":"commandcode/` + model + `","stream":true,"store":false,"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"news?"}]}],
			"tools":[{"type":"function","name":"shell","description":"run","parameters":{"type":"object","properties":{}}},{"type":"web_search","external_web_access":true}]}`
	}
	claude := func(model string) string {
		return `{"model":"commandcode/` + model + `","max_tokens":100,"stream":true,"messages":[{"role":"user","content":"news?"}],
			"tools":[{"name":"Read","description":"read","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search","max_uses":8}]}`
	}
	cases := []struct {
		name, path, body, want string
		native                 bool
	}{
		{"gpt-6-luna", "/v1/responses", codex("gpt-6-luna"), "/v1/responses", true},
		{"xai/grok-4.7", "/v1/responses", codex("xai/grok-4.7"), "/v1/responses", true},
		{"deepseek-v4-pro", "/v1/responses", codex("deepseek/deepseek-v4-pro"), "/v1/responses", false},
		{"claude-sonnet-5-5", "/v1/messages", claude("claude-sonnet-5-5"), "/v1/messages", true},
		{"claude-opus-5-5", "/v1/messages", claude("claude-opus-5-5"), "/v1/messages", false},
	}
	run := func(zdr bool) {
		for _, r := range cases {
			seen()
			if code, b := post(t, r.path, r.body); code != 200 {
				t.Fatalf("%s: %d %s", r.name, code, b)
			}
			paths, bodies := seen()
			if len(paths) != 1 || paths[0] != r.want {
				t.Errorf("%s (zdr %v) went to %v, want %s", r.name, zdr, paths, r.want)
				continue
			}
			got := strings.Contains(bodies[0], `"type":"web_search`)
			if want := r.native && !zdr; got != want {
				t.Errorf("%s (zdr %v): the vendor's own search tool sent = %v, want %v: %s", r.name, zdr, got, want, bodies[0])
			}
		}
	}
	save(nil)
	run(false)
	save(map[string]string{"x-cmd-zdr": "1"})
	run(true)
}
