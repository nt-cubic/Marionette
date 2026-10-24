package gateway

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// With Privacy masking on, a conversation's earlier turns reach the
// vendor as the same bytes on every turn, so its prompt cache is read: a
// value a rule finds in one turn's tool output is masked from then on in
// new text, and the system prompt and the turns already sent with it as
// written go as they went. Masking it there too rewrote the whole prompt
// to the cache on the next turn (audie on Discord: DeepSeek Harness on a
// group with a custom Anthropic provider, 412K to 427K tokens written and
// none read on adjacent turns, ten seconds apart). Here as dsh sends it,
// Chat Completions to a group, and as Claude Code sends it, Messages.
func TestMaskedConversationKeepsItsCachedPrefix(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	var bodies []map[string]json.RawMessage
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var q map[string]json.RawMessage
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &q)
		mu.Lock()
		bodies = append(bodies, q)
		mu.Unlock()
		if string(q["stream"]) == "true" {
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n"+
				"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"+
				"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n"+
				"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n"+
				"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n"+
				"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
			return
		}
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}`)
	}))
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "relay", Name: "Relay", Key: "k", Models: []string{"claude-opus-5-5"}, Anthropic: up.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "flash", Name: "flash", Members: []string{"relay/claude-opus-5-5"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, c := range []struct {
		name, path, secret string
		body               func(turns []string) string
		turn               func(i int, out string) []string
	}{
		{"chat", "/v1/chat/completions", "Hq4cacheDsh81z",
			func(turns []string) string {
				return `{"model":"group/flash","messages":[{"role":"system","content":"You are dsh. Staging deploys with Hq4cacheDsh81z for now."},` +
					`{"role":"user","content":"read the env file"}` + strings.Join(turns, "") + `],` +
					`"tools":[{"type":"function","function":{"name":"bash","description":"run","parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}}}]}`
			},
			func(i int, out string) []string {
				id := fmt.Sprintf("call_%d", i)
				return []string{`,{"role":"assistant","content":"","tool_calls":[{"id":"` + id + `","type":"function","function":{"name":"bash","arguments":"{\"cmd\":\"cat .env\"}"}}]}`,
					`,{"role":"tool","tool_call_id":"` + id + `","content":"` + out + `"}`}
			}},
		{"messages", "/v1/messages", "Wm6cacheCc29qy",
			func(turns []string) string {
				return `{"model":"relay/claude-opus-5-5","max_tokens":1024,"system":"You are Claude. Staging deploys with Wm6cacheCc29qy for now.",` +
					`"tools":[{"name":"bash","description":"run","input_schema":{"type":"object","properties":{"cmd":{"type":"string"}}}}],` +
					`"messages":[{"role":"user","content":"read the env file"}` + strings.Join(turns, "") + `]}`
			},
			func(i int, out string) []string {
				id := fmt.Sprintf("toolu_%d", i)
				return []string{`,{"role":"assistant","content":[{"type":"tool_use","id":"` + id + `","name":"bash","input":{"cmd":"cat .env"}}]}`,
					`,{"role":"user","content":[{"type":"tool_result","tool_use_id":"` + id + `","content":"` + out + `"}]}`}
			}},
	} {
		mu.Lock()
		bodies = nil
		mu.Unlock()
		var turns []string
		for i := range 3 {
			out := "PORT=8080"
			if i == 1 {
				out = "DB_PASSWORD=" + c.secret // a rule finds it here first
			}
			turns = append(turns, c.turn(i, out)...)
			if code, raw := postJSON(t, s, c.path, c.body(turns)); code != 200 {
				t.Fatalf("%s turn %d: %d %s", c.name, i, code, raw)
			}
		}
		mu.Lock()
		got := bodies
		mu.Unlock()
		if len(got) != 3 {
			t.Fatalf("%s: %d requests upstream", c.name, len(got))
		}
		for i := 1; i < len(got); i++ {
			before, now := got[i-1], got[i]
			for _, k := range []string{"system", "tools"} {
				if string(before[k]) != string(now[k]) {
					t.Errorf("%s turn %d: %s changed:\n%s\nwas\n%s", c.name, i, k, now[k], before[k])
				}
			}
			var a, b []json.RawMessage
			json.Unmarshal(before["messages"], &a)
			json.Unmarshal(now["messages"], &b)
			// the turns before, up to the one the cache mark was on
			for j := 0; j < len(a)-1; j++ {
				if string(a[j]) != string(b[j]) {
					t.Errorf("%s turn %d: message %d changed:\n%s\nwas\n%s", c.name, i, j, b[j], a[j])
				}
			}
		}
		last := got[len(got)-1]
		if !strings.Contains(string(last["system"]), c.secret) {
			t.Errorf("%s: the system prompt, sent with the value as written, has it masked now: %s", c.name, last["system"])
		}
		if strings.Contains(string(last["messages"]), c.secret) {
			t.Errorf("%s: the value a rule found went out: %s", c.name, last["messages"])
		}
	}
}
