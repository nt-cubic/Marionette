package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"sync"
	"testing"
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
