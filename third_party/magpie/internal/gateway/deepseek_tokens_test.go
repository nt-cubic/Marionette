package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A chat client that asks DeepSeek for its reply length OpenAI's way
// (max_completion_tokens, as DSH's pi-ai does for a custom provider) gets
// it as max_tokens, the field DeepSeek reads: before, DeepSeek got the
// field it ignores and answered with its default 8K (#2355, h4rk8s).
// Another vendor's chat API gets the request as the client sent it.
func TestDeepSeekChatGetsMaxTokens(t *testing.T) {
	fresh(t)
	f := &fake{reply: sse(
		`data: {"id":"test","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"ok"}}]}`,
		`data: {"id":"test","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`,
		`data: [DONE]`,
	)}
	up := httptest.NewServer(f)
	defer up.Close()
	p, err := provider.FromPreset("deepseek")
	if err != nil {
		t.Fatal(err)
	}
	p.Key = "fixture-key"
	p.Chat = up.URL + "/v1"
	p.Models = []string{"deepseek-flash"}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	other := provider.Provider{ID: "relay", Name: "Relay", Key: "k", Chat: up.URL + "/v1", Models: []string{"m"}}
	if err := provider.Save(other); err != nil {
		t.Fatal(err)
	}
	// a message that names the field keeps its text
	said := `{"role":"user","content":"what is \"max_completion_tokens\"?"}`
	for _, c := range []struct {
		model, fields       string
		maxTokens, maxCompl any
	}{
		{"deepseek/deepseek-flash", `"max_completion_tokens":393216`, float64(393216), nil},
		{"deepseek/deepseek-flash", `"max_completion_tokens" : 5`, float64(5), nil},
		{"deepseek/deepseek-flash", `"max_tokens":5`, float64(5), nil},
		// both named: DeepSeek reads max_tokens, which goes as it is
		{"deepseek/deepseek-flash", `"max_tokens":7,"max_completion_tokens":5`, float64(7), float64(5)},
		{"relay/m", `"max_completion_tokens":5`, nil, float64(5)},
	} {
		body := `{"model":"` + c.model + `","stream":true,` + c.fields + `,"messages":[` + said + `]}`
		code, reply := post(t, "/v1/chat/completions", body)
		if code != 200 {
			t.Fatalf("%s: HTTP %d: %s", c.fields, code, reply)
		}
		var sent map[string]any
		if err := json.Unmarshal(f.got, &sent); err != nil {
			t.Fatal(err)
		}
		if sent["max_tokens"] != c.maxTokens || sent["max_completion_tokens"] != c.maxCompl {
			t.Fatalf("%s %s: upstream got %s", c.model, c.fields, f.got)
		}
		if !strings.Contains(string(f.got), `what is \"max_completion_tokens\"?`) {
			t.Fatalf("%s: message text changed: %s", c.fields, f.got)
		}
	}
}
