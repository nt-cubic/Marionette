package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Factory's plugin, as the built-in (TestFactoryThinkingOff): a model whose
// variants have none is asked for none when an agent turns reasoning off,
// and one whose least is low is asked for low (#899).
func TestPluginThinkingOff(t *testing.T) {
	t.Setenv("FAKE_OFF", "1")
	var mu sync.Mutex
	var sent []any
	pid := besideFake(t, "factory", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var q map[string]any
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q["reasoning_effort"])
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(`data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}`, `data: [DONE]`))
	}))
	for _, tt := range []struct {
		name, path, body string
		want             any
	}{
		{"chat none", "/v1/chat/completions", `{"model":"` + pid + `/fake-off","reasoning_effort":"none","messages":[{"role":"user","content":"hi"}]}`, "none"},
		{"responses none", "/v1/responses", `{"model":"` + pid + `/fake-off","reasoning":{"effort":"none"},"input":[{"role":"user","content":"hi"}]}`, "none"},
		{"messages disabled", "/v1/messages", `{"model":"` + pid + `/fake-off","thinking":{"type":"disabled"},"messages":[{"role":"user","content":"hi"}],"max_tokens":8}`, "none"},
		{"chat low", "/v1/chat/completions", `{"model":"` + pid + `/fake-off","reasoning_effort":"low","messages":[{"role":"user","content":"hi"}]}`, "low"},
		// a client's "off" is none (plugins#334), in either API and as a
		// model's suffix, never sent on as "off" nor as the least level on
		{"chat off", "/v1/chat/completions", `{"model":"` + pid + `/fake-off","reasoning_effort":"off","messages":[{"role":"user","content":"hi"}]}`, "none"},
		{"chat OFF spaced", "/v1/chat/completions", `{"model":"` + pid + `/fake-off","reasoning_effort" : " OFF ","messages":[{"role":"user","content":"hi"}]}`, "none"},
		{"responses off", "/v1/responses", `{"model":"` + pid + `/fake-off","reasoning":{"effort":"off","summary":"auto"},"input":[{"role":"user","content":"hi"}]}`, "none"},
		{"suffix off", "/v1/chat/completions", `{"model":"` + pid + `/fake-off:off","messages":[{"role":"user","content":"hi"}]}`, "none"},
		{"no off, off", "/v1/chat/completions", `{"model":"` + pid + `/fake-on","reasoning_effort":"off","messages":[{"role":"user","content":"hi"}]}`, "low"},
		{"no off", "/v1/responses", `{"model":"` + pid + `/fake-on","reasoning":{"effort":"none"},"input":[{"role":"user","content":"hi"}]}`, "low"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			mu.Lock()
			sent = nil
			mu.Unlock()
			postProto(t, New(), tt.path, tt.body)
			mu.Lock()
			defer mu.Unlock()
			if len(sent) != 1 || sent[0] != tt.want {
				t.Errorf("reasoning_effort sent %v, want %v", sent, tt.want)
			}
		})
	}
}

// TestOffAsNone: only an effort spelled "off" changes, to none; the rest
// of the body, numbers included, goes as it came, and another level or
// an "off" that isn't an effort is left alone (plugins#334).
func TestOffAsNone(t *testing.T) {
	for _, tt := range []struct {
		name     string
		from     provider.Protocol
		in, want string
	}{
		{"chat", provider.Chat, `{"model":"m","reasoning_effort":"off","max_tokens":12345678901}`, `{"max_tokens":12345678901,"model":"m","reasoning_effort":"none"}`},
		{"responses", provider.Responses, `{"model":"m","reasoning":{"effort":"Off","summary":"auto","max_tokens":4096}}`, `{"model":"m","reasoning":{"effort":"none","max_tokens":4096,"summary":"auto"}}`},
		{"chat reasoning object", provider.Chat, `{"model":"m","reasoning":{"effort":"off"}}`, `{"model":"m","reasoning":{"effort":"none"}}`},
		{"low", provider.Chat, `{"model":"m","reasoning_effort":"low"}`, `{"model":"m","reasoning_effort":"low"}`},
		{"offline", provider.Chat, `{"model":"m","reasoning_effort":"offline"}`, `{"model":"m","reasoning_effort":"offline"}`},
		{"off in text", provider.Chat, `{"model":"m","messages":[{"role":"user","content":"effort\": \"off\""}]}`, `{"model":"m","messages":[{"role":"user","content":"effort\": \"off\""}]}`},
		{"anthropic", provider.Anthropic, `{"model":"m","output_config":{"effort":"off"}}`, `{"model":"m","output_config":{"effort":"off"}}`},
	} {
		t.Run(tt.name, func(t *testing.T) {
			got := strings.TrimSpace(string(offAsNone(tt.from, []byte(tt.in))))
			if got != tt.want {
				t.Errorf("offAsNone(%s) = %s, want %s", tt.in, got, tt.want)
			}
		})
	}
}

// TestAskedAtOff: a model asked ":off" is asked at none, as ":none" is.
func TestAskedAtOff(t *testing.T) {
	for _, id := range []string{"group/g:off", "group/g:OFF"} {
		if m, e := askedAt(id); m != "group/g" || e != "none" {
			t.Errorf("askedAt(%q) = %q, %q; want group/g, none", id, m, e)
		}
	}
	if m, e := askedAt("group/g:offline"); m != "group/g:offline" || e != "" {
		t.Errorf("askedAt(group/g:offline) = %q, %q", m, e)
	}
}
