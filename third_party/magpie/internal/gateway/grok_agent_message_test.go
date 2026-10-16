package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// agentMessageTurn is a lead's turn with its subagent's plain reply in it,
// between the call that sent the task and its output, as Codex's
// MultiAgentV2 hands it back, with no tools.
const agentMessageTurn = `"input":[
	{"type":"message","role":"user","content":"go"},
	{"type":"function_call","call_id":"c1","name":"send_message","arguments":"{}"},
	{"type":"agent_message","author":"/root/research","recipient":"/root","content":[{"type":"input_text","text":"Message Type: FINAL_ANSWER\nPayload:\nfound the cause"}]},
	{"type":"function_call_output","call_id":"c1","output":"sent"}]`

// checkAgentMessageSent fails unless Grok was given the subagent's reply
// as the user's message, after the run it broke, its sender and recipient
// first. The reply sat between the call and its output on the way in;
// joinToolRuns puts it after the output, so the exchange Grok reads is
// whole. Its text, its role, and its conversion are as they were.
func checkAgentMessageSent(t *testing.T, q map[string]any) {
	t.Helper()
	in, _ := q["input"].([]any)
	if len(in) != 4 {
		t.Fatalf("Grok was given %v", q["input"])
	}
	if o, _ := in[2].(map[string]any); o["type"] != "function_call_output" || o["call_id"] != "c1" {
		t.Fatalf("its call's output went as %v", in[2])
	}
	m, _ := in[3].(map[string]any)
	got, _ := json.Marshal(m)
	if want := `{"content":[{"text":"From /root/research to /root\n\n","type":"input_text"},{"text":"Message Type: FINAL_ANSWER\nPayload:\nfound the cause","type":"input_text"}],"role":"user","type":"message"}`; string(got) != want {
		t.Fatalf("the agent_message went as\n%s\nwant\n%s", got, want)
	}
}

// A Codex subagent's plain agent_message reaches a Grok subscription as the
// user's message, which its backend refused with 422 "unknown item type
// \"agent_message\"" (plugins#61).
func TestGrokGetsAgentMessageAsUser(t *testing.T) {
	grokSignedIn(t)
	var mu sync.Mutex
	var sent []map[string]any
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/responses" {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, `{"data":[{"id":"grok-4.7","api_backend":"responses"}]}`)
			return
		}
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		done := `{"id":"r1","object":"response","status":"completed","output":[{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"ok","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":1}}`
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, done)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":`+done+`}`))
	}))
	defer up.Close()
	was := provider.GrokBase
	provider.GrokBase = up.URL + "/v1"
	defer func() { provider.GrokBase = was }()

	s := New()
	for _, stream := range []string{"true", "false"} {
		mu.Lock()
		sent = nil
		mu.Unlock()
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"grok/grok-4.7","stream":`+stream+`,`+agentMessageTurn+`}`)))
		if rec.Code != 200 {
			t.Fatalf("stream %s: %d %s", stream, rec.Code, rec.Body)
		}
		mu.Lock()
		if len(sent) != 1 {
			t.Fatalf("stream %s: Grok was asked %d times", stream, len(sent))
		}
		q := sent[0]
		mu.Unlock()
		checkAgentMessageSent(t, q)
	}
}
