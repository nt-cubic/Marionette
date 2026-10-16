package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

// zedParses is what Zed's cloud asks of an OpenAI Responses request
// (zed-industries/zed, crates/open_ai/src/responses.rs) before it goes on,
// "" when it would take it.
func zedParses(q map[string]any) string {
	items, _ := q["input"].([]any)
	for _, it := range items {
		im, _ := it.(map[string]any)
		switch ty, _ := im["type"].(string); ty {
		case "message":
			switch im["role"] {
			case "user", "assistant", "system", "tool":
			default:
				return "unknown variant `" + im["role"].(string) + "`, expected one of `user`, `assistant`, `system`, `tool`"
			}
			if _, ok := im["content"].([]any); !ok {
				return "invalid type: expected a sequence"
			}
		case "reasoning":
			if c, ok := im["content"]; ok && c == nil {
				return "invalid type: null, expected a sequence"
			}
		case "function_call", "function_call_output", "custom_tool_call", "custom_tool_call_output", "compaction", "compaction_trigger":
		default:
			return "unknown variant `" + ty + "`"
		}
	}
	tools, _ := q["tools"].([]any)
	for _, t := range tools {
		if ty := t.(map[string]any)["type"]; ty != "function" && ty != "custom" {
			return "unknown variant `" + ty.(string) + "`, expected `function` or `custom`"
		}
	}
	inc, _ := q["include"].([]any)
	for _, v := range inc {
		if v != "reasoning.encrypted_content" {
			return "unknown variant `" + v.(string) + "`"
		}
	}
	return ""
}

// A Codex request on an OpenAI model of Zed's plugin reaches Zed as its
// cloud reads it: the developer messages Codex sends at the start and
// mid-conversation (a model switched) as system ones where they stand,
// reasoning with a null content without it, a web search an OpenAI model
// ran left out, and the namespaced tools flat (nico on Discord: "failed to
// parse OpenAI Responses API request: unknown variant `developer`").
func TestZedPluginGetsNoDeveloperRole(t *testing.T) {
	var mu sync.Mutex
	var sent []map[string]any
	pid := besideFake(t, "zed", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		if why := zedParses(q); why != "" {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(400)
			io.WriteString(w, `{"error":{"message":"failed to parse OpenAI Responses API request: `+why+`"}}`)
			return
		}
		done := `{"id":"r1","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":5,"output_tokens":1}}`
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, done)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","item_id":"m1","output_index":0,"delta":"ok"}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":`+done+`}`))
	}))
	body := `{"model":"` + pid + `/fake-resp","stream":true,"instructions":"You are Codex.",` + namespacedTools + `,
		"include":["reasoning.encrypted_content"],"reasoning":{"effort":"medium","summary":"auto"},"input":[
		{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>sandboxed</permissions instructions>"}]},
		{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
		{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking"}],"content":null,"encrypted_content":"gAAAAAx"},
		{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"magpie"}},
		{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]},
		{"type":"message","role":"developer","content":[{"type":"input_text","text":"<model_switch>now another model</model_switch>"}]},
		{"type":"message","role":"user","content":[{"type":"input_text","text":"again"}]}]}`
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(sent) != 1 {
		t.Fatalf("asked %d times", len(sent))
	}
	q := sent[0]
	var roles []string
	for _, it := range q["input"].([]any) {
		im := it.(map[string]any)
		if im["type"] == "message" {
			roles = append(roles, im["role"].(string))
		}
	}
	if got := strings.Join(roles, ","); got != "system,user,assistant,system,user" {
		t.Fatalf("roles %s", got)
	}
	in := q["input"].([]any)
	if len(in) != 6 {
		t.Fatalf("input %v", in)
	}
	if s := in[4].(map[string]any)["content"].([]any)[0].(map[string]any)["text"]; s != "<model_switch>now another model</model_switch>" {
		t.Fatalf("the model switch went as %v", in[4])
	}
	if r := in[2].(map[string]any); r["type"] != "reasoning" || r["encrypted_content"] != "gAAAAAx" {
		t.Fatalf("reasoning went as %v", r)
	}
	if q["instructions"] != "You are Codex." {
		t.Fatalf("instructions %v", q["instructions"])
	}
	var names []string
	for _, tl := range q["tools"].([]any) {
		tm := tl.(map[string]any)
		names = append(names, tm["type"].(string)+":"+tm["name"].(string))
	}
	if got := strings.Join(names, ","); got != "function:exec_command,function:collaboration__spawn_agent" {
		t.Fatalf("Zed was offered %s", got)
	}
}

// The built-in Zed account's OpenAI request is fitted the same way: a web
// search offered (no search API set up) leaves no web_search tool, nor
// include's web_search_call.action.sources, for Zed's cloud to turn away.
func TestZedBuiltInResponsesParse(t *testing.T) {
	req, err := parseResponses([]byte(`{"model":"m","stream":true,"include":["reasoning.encrypted_content"],"reasoning":{"effort":"high"},
		"tools":[{"type":"function","name":"exec_command","parameters":{"type":"object"}},{"type":"web_search"}],"input":[
		{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]},
		{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}]}`))
	if err != nil {
		t.Fatal(err)
	}
	raw, _, err := zedRequest(req, "open_ai", "gpt-5.5")
	if err != nil {
		t.Fatal(err)
	}
	var q map[string]any
	json.Unmarshal(raw, &q)
	if why := zedParses(q); why != "" {
		t.Fatalf("%s: %s", why, raw)
	}
}
