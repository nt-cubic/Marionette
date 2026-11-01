package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/yetone/magpie/internal/provider"
)

// packinggoodsllc_07668 on Discord (#general, Claude Code v2.1.316 through
// the routing group "Hard to strong, easy to cheap"): the reply, word for
// word, as Claude Code showed it. The model wrote its calls as invokes in
// its text, with the results it made up after each, before anything ran.
const invokeReported = "<invoke name=\"ToolSearch\">\n" +
	"<parameter name=\"query\">select:mcp__magpie__magpie-browser__browser_navigate</parameter>\n" +
	"</invoke>\n" +
	"*Result: Tool loaded.*\n" +
	"<invoke name=\"mcp__magpie__magpie-browser__browser_navigate\">\n" +
	"<parameter name=\"url\">https://www.google.com</parameter>\n" +
	"</invoke>\n" +
	"*Result: Navigated to https://www.google.com — page title: \"Google\"*\n" +
	"已用 magpie 的浏览器打开了谷歌 ..."

const invokeQuery = `{"query":"select:mcp__magpie__magpie-browser__browser_navigate"}`

// Claude Code's tools, the browser's deferred: ToolSearch loads it.
const invokeAnthropicTools = `"tools":[{"name":"ToolSearch","input_schema":{"type":"object"}},{"name":"Bash","input_schema":{"type":"object"}}]`

// cutInto is s in pieces of n bytes at most, cut on rune boundaries.
func cutInto(s string, n int) []string {
	var out []string
	for s != "" {
		i := min(n, len(s))
		for i < len(s) && !utf8.RuneStart(s[i]) {
			i++
		}
		out = append(out, s[:i])
		s = s[i:]
	}
	return out
}

// The reported reply is the ToolSearch call, and what the model made up
// after it goes: the browser call after it depends on a result that
// didn't exist yet, and names a tool not offered.
func TestInvokeReportedParsed(t *testing.T) {
	names := map[string]bool{"ToolSearch": true, "Bash": true}
	calls, rest, ok := parseTextCalls(invokeReported, names)
	if !ok || len(calls) != 1 || calls[0].Name != "ToolSearch" || string(calls[0].Args) != invokeQuery || rest != "" {
		t.Fatalf("calls %+v rest %q ok %v", calls, rest, ok)
	}
	// the browser offered too: still the first call only, the second
	// came after a made-up result
	names["mcp__magpie__magpie-browser__browser_navigate"] = true
	calls, rest, ok = parseTextCalls(invokeReported, names)
	if !ok || len(calls) != 1 || calls[0].Name != "ToolSearch" || rest != "" {
		t.Fatalf("browser offered: calls %+v rest %q ok %v", calls, rest, ok)
	}
	// calls back to back are all made
	two := "<invoke name=\"Bash\">\n<parameter name=\"command\">ls</parameter>\n</invoke>\n" +
		"<invoke name=\"Bash\">\n<parameter name=\"command\">pwd</parameter>\n</invoke>"
	calls, rest, ok = parseTextCalls(two, names)
	if !ok || len(calls) != 2 || string(calls[1].Args) != `{"command":"pwd"}` || rest != "" {
		t.Fatalf("two: calls %+v rest %q ok %v", calls, rest, ok)
	}
	// MiniMax's wrapper and Claude's
	for _, w := range [][2]string{{"<minimax:tool_call>", "</minimax:tool_call>"}, {"<function_calls>", "</function_calls>"}} {
		s := w[0] + "\n<invoke name=\"Bash\">\n<parameter name=\"command\">ls -la</parameter>\n</invoke>\n" + w[1]
		if at, _ := textCallAt("先看看。\n" + s); at != len("先看看。\n") {
			t.Fatalf("%s: not seen (%d)", w[0], at)
		}
		calls, rest, ok = parseTextCalls(s, names)
		if !ok || len(calls) != 1 || string(calls[0].Args) != `{"command":"ls -la"}` || rest != "" {
			t.Fatalf("%s: calls %+v rest %q ok %v", w[0], calls, rest, ok)
		}
	}
	// an invoke of a tool not offered stays text
	if _, _, ok := parseTextCalls("<invoke name=\"rm_rf\">\n<parameter name=\"p\">/</parameter>\n</invoke>", names); ok {
		t.Fatal("unoffered tool read as a call")
	}
}

// Through a Chat relay, streamed in pieces: the call, and none of the
// made-up text.
func TestInvokeReportedRelayed(t *testing.T) {
	var up []string
	for _, p := range cutInto(invokeReported, 7) {
		up = append(up, chunkOf(p))
	}
	up = append(up, stopChunk)
	tools := `"tools":[{"type":"function","function":{"name":"ToolSearch","parameters":{"type":"object"}}}]`
	text, calls, finish, raw := relayedChat(t, `{"model":"a/m","stream":true,"messages":[{"role":"user","content":"帮我使用magpie的browser打开谷歌"}],`+tools+`}`, up...)
	if text != "" || calls["ToolSearch"] != invokeQuery || len(calls) != 1 || finish != "tool_calls" || strings.Contains(raw, "Result") || strings.Contains(raw, "invoke") {
		t.Fatalf("text %q calls %v finish %q\n%s", text, calls, finish, raw)
	}
}

// anthropicSSE is a Messages stream whose one text block says text, in
// pieces.
func anthropicSSE(text string) string {
	ev := func(typ string, v any) string {
		b, _ := json.Marshal(v)
		return "event: " + typ + "\ndata: " + string(b)
	}
	lines := []string{
		ev("message_start", map[string]any{"type": "message_start", "message": map[string]any{"id": "msg_1", "type": "message", "role": "assistant", "model": "m", "content": []any{}, "usage": map[string]any{"input_tokens": 9, "output_tokens": 1}}}),
		ev("content_block_start", map[string]any{"type": "content_block_start", "index": 0, "content_block": map[string]any{"type": "text", "text": ""}}),
	}
	for _, p := range cutInto(text, 9) {
		lines = append(lines, ev("content_block_delta", map[string]any{"type": "content_block_delta", "index": 0, "delta": map[string]any{"type": "text_delta", "text": p}}))
	}
	lines = append(lines,
		ev("content_block_stop", map[string]any{"type": "content_block_stop", "index": 0}),
		ev("message_delta", map[string]any{"type": "message_delta", "delta": map[string]any{"stop_reason": "end_turn"}, "usage": map[string]any{"output_tokens": 40}}),
		ev("message_stop", map[string]any{"type": "message_stop"}))
	return sse(lines...)
}

// anthropicRead is what a Messages client reads of a stream: the text,
// each tool_use's name and input, and the stop reason. Blocks must open
// and close in order, each index once.
func anthropicRead(t *testing.T, raw string) (text string, names []string, inputs []string, stop string) {
	t.Helper()
	open, next := -1, 0
	for _, line := range strings.Split(raw, "\n") {
		data, ok := strings.CutPrefix(line, "data: ")
		if !ok {
			continue
		}
		var ev struct {
			Type         string `json:"type"`
			Index        int    `json:"index"`
			ContentBlock struct {
				Type string `json:"type"`
				Name string `json:"name"`
			} `json:"content_block"`
			Delta struct {
				Type        string `json:"type"`
				Text        string `json:"text"`
				PartialJSON string `json:"partial_json"`
				StopReason  string `json:"stop_reason"`
			} `json:"delta"`
		}
		if err := json.Unmarshal([]byte(data), &ev); err != nil {
			t.Fatalf("unreadable event %q: %v", data, err)
		}
		switch ev.Type {
		case "content_block_start":
			if open >= 0 || ev.Index != next {
				t.Fatalf("block %d opened (open %d, next %d)\n%s", ev.Index, open, next, raw)
			}
			open, next = ev.Index, next+1
			if ev.ContentBlock.Type == "tool_use" {
				names, inputs = append(names, ev.ContentBlock.Name), append(inputs, "")
			}
		case "content_block_delta":
			if ev.Index != open {
				t.Fatalf("delta for %d, %d open\n%s", ev.Index, open, raw)
			}
			text += ev.Delta.Text
			if ev.Delta.Type == "input_json_delta" {
				inputs[len(inputs)-1] += ev.Delta.PartialJSON
			}
		case "content_block_stop":
			if ev.Index != open {
				t.Fatalf("stop for %d, %d open\n%s", ev.Index, open, raw)
			}
			open = -1
		case "message_delta":
			stop = ev.Delta.StopReason
		}
	}
	return text, names, inputs, stop
}

// The reported case on its own path: Claude Code's Messages request to a
// routing group whose model speaks Messages itself, relayed as it came.
// It reaches Claude Code as the ToolSearch call, and the turn stops for
// it; a reply that came whole the same.
func TestInvokeReportedAnthropicPassthrough(t *testing.T) {
	fresh(t)
	a := &scripted{}
	scriptedOn(t, "a", provider.Anthropic, a)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	ask := func(stream bool) string {
		rec := httptest.NewRecorder()
		s := "false"
		if stream {
			s = "true"
		}
		New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(
			`{"model":"group/g","max_tokens":100,"stream":`+s+`,"messages":[{"role":"user","content":"帮我使用magpie的browser打开谷歌"}],`+invokeAnthropicTools+`}`)))
		if rec.Code != 200 {
			t.Fatalf("%d %s", rec.Code, rec.Body.String())
		}
		return rec.Body.String()
	}

	a.replies, a.n = []reply{{200, "text/event-stream", anthropicSSE(invokeReported)}}, 0
	raw := ask(true)
	text, names, inputs, stop := anthropicRead(t, raw)
	if text != "" || len(names) != 1 || names[0] != "ToolSearch" || inputs[0] != invokeQuery || stop != "tool_use" || strings.Contains(raw, "Result") {
		t.Fatalf("text %q names %v inputs %v stop %q\n%s", text, names, inputs, stop, raw)
	}

	// text before the call stays, in its block, and the call follows it
	a.replies, a.n = []reply{{200, "text/event-stream", anthropicSSE("我来打开谷歌。\n\n" + invokeReported)}}, 0
	raw = ask(true)
	text, names, inputs, stop = anthropicRead(t, raw)
	if text != "我来打开谷歌。\n\n" || len(names) != 1 || inputs[0] != invokeQuery || stop != "tool_use" {
		t.Fatalf("lead: text %q names %v inputs %v stop %q\n%s", text, names, inputs, stop, raw)
	}

	// text that only names an invoke of no offered tool goes as it came
	plain := "Write it as <invoke name=\"rm_rf\"><parameter name=\"p\">/</parameter></invoke>, then stop."
	a.replies, a.n = []reply{{200, "text/event-stream", anthropicSSE(plain)}}, 0
	raw = ask(true)
	text, names, _, stop = anthropicRead(t, raw)
	if text != plain || len(names) != 0 || stop != "end_turn" {
		t.Fatalf("plain: text %q names %v stop %q\n%s", text, names, stop, raw)
	}

	whole, _ := json.Marshal(map[string]any{"id": "msg_1", "type": "message", "role": "assistant", "model": "m",
		"content":     []any{map[string]any{"type": "text", "text": "好的。\n" + invokeReported}},
		"stop_reason": "end_turn", "usage": map[string]any{"input_tokens": 9, "output_tokens": 40}})
	a.replies, a.n = []reply{{200, "application/json", string(whole)}}, 0
	raw = ask(false)
	var got struct {
		Content []struct {
			Type  string          `json:"type"`
			Text  string          `json:"text"`
			Name  string          `json:"name"`
			ID    string          `json:"id"`
			Input json.RawMessage `json:"input"`
		} `json:"content"`
		StopReason string `json:"stop_reason"`
	}
	if err := json.Unmarshal([]byte(raw), &got); err != nil {
		t.Fatalf("%v: %s", err, raw)
	}
	if len(got.Content) != 2 || got.Content[0].Text != "好的。" || got.Content[1].Type != "tool_use" || got.Content[1].Name != "ToolSearch" ||
		string(got.Content[1].Input) != invokeQuery || got.Content[1].ID == "" || got.StopReason != "tool_use" {
		t.Fatalf("whole: %s", raw)
	}
}

// Translated, the reported reply from a Chat model reaches a Messages
// client (Claude Code) as the call, streamed and whole.
func TestInvokeReportedTranslated(t *testing.T) {
	fresh(t)
	var up []string
	for _, p := range cutInto(invokeReported, 5) {
		up = append(up, chunkOf(p))
	}
	body := sse(append(up, stopChunk, `data: [DONE]`)...)
	a := &scripted{}
	scriptedOn(t, "a", provider.Chat, a)
	for _, stream := range []bool{true, false} {
		a.replies, a.n = []reply{{200, "text/event-stream", body}}, 0
		s := "false"
		if stream {
			s = "true"
		}
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(
			`{"model":"a/m","max_tokens":100,"stream":`+s+`,"messages":[{"role":"user","content":"hi"}],`+invokeAnthropicTools+`}`)))
		raw := rec.Body.String()
		if rec.Code != 200 || strings.Contains(raw, "invoke") || strings.Contains(raw, "Result") || !strings.Contains(raw, `"stop_reason":"tool_use"`) || !strings.Contains(raw, `"name":"ToolSearch"`) {
			t.Fatalf("stream=%v %d %s", stream, rec.Code, raw)
		}
	}
}
