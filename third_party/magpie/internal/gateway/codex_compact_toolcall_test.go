package gateway

import (
	"bytes"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Lucy on Discord (v0.1.1676, Codex, group/auto-deepseek-v4-1-flash):
// "compaction: group/auto-deepseek-v4-1-flash failed to summarise this
// conversation 3 times in a row (last: the model wrote no summary (it
// answered with reasoning, function_call exec_command))". The summary
// request offered no tools, but its history was Codex's exchange of
// exec_command calls, and DeepSeek went on with it: it thought, then called
// exec_command again rather than writing the summary.
//
// The upstreams here answer as DeepSeek did, in each protocol's own shape:
// reasoning and an exec_command call while the history it is given holds
// calls, the summary only when it doesn't.

const compactExecRequest = `{"model":"MODEL","stream":true,"store":false,"tools":[],"tool_choice":"auto",
  "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the failing test in pkg/calc"}]},
  {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"run the tests"}]},
  {"type":"function_call","id":"fc_1","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"go test ./pkg/calc/...\"}"},
  {"type":"function_call_output","call_id":"call_1","output":"--- FAIL: TestAdd: Add(2,2) = 5, want 4"},
  {"type":"function_call","id":"fc_2","call_id":"call_2","name":"exec_command","arguments":"{\"cmd\":\"sed -n 1,40p pkg/calc/calc.go\"}"},
  {"type":"function_call_output","call_id":"call_2","output":"func Add(a, b int) int { return a + b + 1 }"},
  {"type":"compaction_trigger"}]}`

// deepseekCalls is DeepSeek's reply calling exec_command, as proto streams
// it; deepseekSummary its summary.
func deepseekCalls(proto provider.Protocol) string {
	switch proto {
	case provider.Chat:
		return sse(
			`data: {"id":"c1","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":"The test still fails; run it again to check."},"finish_reason":null}]}`,
			`data: {"id":"c1","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_00_x1","type":"function","function":{"name":"exec_command","arguments":""}}]},"finish_reason":null}]}`,
			`data: {"id":"c1","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"cmd\":\"go test ./...\"}"}}]},"finish_reason":null}]}`,
			`data: {"id":"c1","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":900,"completion_tokens":40,"total_tokens":940}}`,
			`data: [DONE]`)
	case provider.Anthropic:
		return sse(
			`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"stop_reason":null,"usage":{"input_tokens":900,"output_tokens":0}}}`,
			`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}`,
			`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"The test still fails; run it again to check."}}`,
			`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
			`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_00_x1","name":"exec_command","input":{}}}`,
			`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"go test ./...\"}"}}`,
			`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":1}`,
			`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":40}}`,
			`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
	}
	return sse(
		`data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}`,
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_x","summary":[{"type":"summary_text","text":"The test still fails; run it again to check."}]}}`,
		`data: {"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_x","call_id":"call_00_x1","name":"exec_command","arguments":"{\"cmd\":\"go test ./...\"}","status":"completed"}}`,
		`data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[],"usage":{"input_tokens":900,"output_tokens":40}}}`)
}

func deepseekSummary(proto provider.Protocol) string {
	switch proto {
	case provider.Chat:
		return sse(
			`data: {"id":"c2","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning_content":"Summarise."},"finish_reason":null}]}`,
			`data: {"id":"c2","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"content":"SUMMARY: Add adds 1 too many"},"finish_reason":null}]}`,
			`data: {"id":"c2","object":"chat.completion.chunk","model":"deepseek-flash","choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":800,"completion_tokens":20,"total_tokens":820}}`,
			`data: [DONE]`)
	case provider.Anthropic:
		return sse(
			`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_2","type":"message","role":"assistant","model":"deepseek-flash","content":[],"stop_reason":null,"usage":{"input_tokens":800,"output_tokens":0}}}`,
			`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
			`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"SUMMARY: Add adds 1 too many"}}`,
			`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
			`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}}`,
			`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
	}
	return sse(
		`data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"SUMMARY: Add adds 1 too many"}]}}`,
		`data: {"type":"response.completed","response":{"id":"resp_2","status":"completed","output":[],"usage":{"input_tokens":800,"output_tokens":20}}}`)
}

// historyCalls is whether a request in proto carries an exchange of calls
// in its history, as that protocol writes one.
func historyCalls(proto provider.Protocol, b []byte) bool {
	switch proto {
	case provider.Chat:
		return bytes.Contains(b, []byte(`"tool_calls"`))
	case provider.Anthropic:
		return bytes.Contains(b, []byte(`"tool_use"`))
	}
	return bytes.Contains(b, []byte(`"function_call"`))
}

// deepseekUpstream serves proto at a test server as DeepSeek answered:
// calls while the history holds calls (always, when stubborn).
func deepseekUpstream(t *testing.T, proto provider.Protocol, stubborn bool) (*httptest.Server, func() [][]byte) {
	var mu sync.Mutex
	var got [][]byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = append(got, b)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		if stubborn || historyCalls(proto, b) {
			io.WriteString(w, deepseekCalls(proto))
			return
		}
		io.WriteString(w, deepseekSummary(proto))
	}))
	t.Cleanup(up.Close)
	return up, func() [][]byte {
		mu.Lock()
		defer mu.Unlock()
		return append([][]byte(nil), got...)
	}
}

// saveDeepSeek saves a provider "ds" serving deepseek-flash over proto at
// up, and the group auto-ds of it.
func saveDeepSeek(t *testing.T, proto provider.Protocol, up string) {
	t.Helper()
	noRests()
	t.Cleanup(noRests)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	p := provider.Provider{ID: "ds", Name: "DS", Key: "k", Models: []string{"deepseek-flash"}}
	switch proto {
	case provider.Chat:
		p.Chat = up + "/v1"
	case provider.Responses:
		p.Responses = up + "/v1"
	case provider.Anthropic:
		p.Anthropic = up
	}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "auto-ds", Name: "auto-ds", Members: []string{"ds/deepseek-flash"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
}

var compactProtos = []provider.Protocol{provider.Chat, provider.Responses, provider.Anthropic}

func TestCodexCompactToolCallAsksAgainAsText(t *testing.T) {
	for _, proto := range compactProtos {
		for _, model := range []string{"group/auto-ds", "ds/deepseek-flash"} {
			t.Run(string(proto)+"/"+model, func(t *testing.T) {
				up, got := deepseekUpstream(t, proto, false)
				saveDeepSeek(t, proto, up.URL)
				code, body := codexPost(t, strings.Replace(compactExecRequest, "MODEL", model, 1))
				if code != 200 {
					t.Fatalf("%d %s", code, body)
				}
				if s := compactedText(t, body); s != "SUMMARY: Add adds 1 too many" {
					t.Errorf("summary %q", s)
				}
				asked := got()
				if len(asked) != 2 {
					t.Fatalf("asked %d times, not twice", len(asked))
				}
				// the first as Codex's conversation, the second as its text,
				// with every call and output in it and Codex's prompt after
				if !historyCalls(proto, asked[0]) {
					t.Errorf("the first summary request lost the calls: %s", asked[0])
				}
				again := string(asked[1])
				for _, want := range []string{"fix the failing test in pkg/calc", "exec_command", "go test ./pkg/calc/...", "Add(2,2) = 5, want 4", "a + b + 1", "CONTEXT CHECKPOINT COMPACTION"} {
					if !strings.Contains(again, want) {
						t.Errorf("the request asked again lacks %q: %s", want, again)
					}
				}
			})
		}
	}
}

// A model that calls a tool however it is asked: compacted without a
// model, with why, so Codex goes on instead of being held for minutes.
func TestCodexCompactToolCallEveryTimeCompactsLocally(t *testing.T) {
	for _, proto := range compactProtos {
		t.Run(string(proto), func(t *testing.T) {
			up, got := deepseekUpstream(t, proto, true)
			saveDeepSeek(t, proto, up.URL)
			code, body := codexPost(t, strings.Replace(compactExecRequest, "MODEL", "group/auto-ds", 1))
			if code != 200 {
				t.Fatalf("%d %s", code, body)
			}
			s := compactedText(t, body)
			for _, want := range []string{"without a model", "fix the failing test in pkg/calc", "function_call exec_command"} {
				if !strings.Contains(s, want) {
					t.Errorf("local summary lacks %q:\n%s", want, s)
				}
			}
			if n := len(got()); n != 2 {
				t.Errorf("asked %d times, not twice", n)
			}
		})
	}
}
