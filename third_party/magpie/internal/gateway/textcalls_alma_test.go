package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// wishSeikouudoku on X (2026-11-02): Alma, through a magpie group that
// sends it on to DeepSeek, got <｜｜DSML｜｜ calls>, invoke name="…" and
// parameter as its reply's text, and no tool ran. Alma's provider of type
// openai speaks Responses (/v1/responses, streamed), and DeepSeek's preset
// speaks Responses too, so the reply is relayed as it came: the route
// #4743's responsesTextCalls reads. The stream here is DeepSeek-shaped: a
// reasoning item first, then the message whose text is observedPipes
// (the double-pipe calls DeepSeek wrote in real use), one character to a
// delta so every mark is cut across events. The group's first member
// answers, or refuses and the second does.
func TestDSMLAlmaResponsesThroughGroup(t *testing.T) {
	stream := dsmlResponsesSSE("我先搜一下。\n\n" + observedPipes)
	part := map[string]any{"type": "output_text", "text": "我先搜一下。\n\n" + observedPipes, "annotations": []any{}}
	whole, _ := json.Marshal(map[string]any{"id": "r1", "object": "response", "model": "deepseek-flash", "status": "completed",
		"output": []any{
			map[string]any{"type": "reasoning", "id": "rs_1", "summary": []any{map[string]any{"type": "summary_text", "text": "need a search"}}},
			map[string]any{"type": "message", "role": "assistant", "id": "msg_1", "status": "completed", "content": []any{part}}}})
	refused := reply{500, "application/json", `{"error":{"message":"upstream overloaded"}}`}
	// Alma's request as the AI SDK's openai provider writes it
	body := func(stream bool) string {
		s := "false"
		if stream {
			s = "true"
		}
		return `{"model":"group/ds","stream":` + s + `,"input":[{"role":"system","content":"You are Alma."},{"role":"user","content":[{"type":"input_text","text":"What is the latest Python?"}]}],` +
			`"tools":[{"type":"function","name":"web_search","description":"Search the web","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},"strict":false}],"tool_choice":"auto"}`
	}
	const wantArgs = `{"query":"current Python version latest stable release"}`
	for _, streamed := range []bool{true, false} {
		for _, first := range []bool{true, false} {
			fresh(t)
			ok := reply{200, "text/event-stream", stream}
			if !streamed {
				ok = reply{200, "application/json", string(whole)}
			}
			a, b := &scripted{replies: []reply{ok}}, &scripted{replies: []reply{ok}}
			if !first {
				a.replies = []reply{refused}
			}
			scriptedOn(t, "a", provider.Responses, a)
			scriptedOn(t, "b", provider.Responses, b)
			if err := provider.SaveGroup(provider.Group{ID: "ds", Name: "DeepSeek", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			rec := httptest.NewRecorder()
			New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body(streamed))))
			raw := rec.Body.String()
			var text string
			var names, args []string
			if streamed {
				text, names, args, _, _ = responsesStreamRead(t, raw)
			} else {
				var got struct {
					Output []struct {
						Type    string `json:"type"`
						Name    string `json:"name"`
						Args    string `json:"arguments"`
						Content []struct {
							Text string `json:"text"`
						} `json:"content"`
					} `json:"output"`
				}
				if err := json.Unmarshal([]byte(raw), &got); err != nil {
					t.Fatalf("%v: %s", err, raw)
				}
				for _, o := range got.Output {
					switch o.Type {
					case "message":
						for _, c := range o.Content {
							text += c.Text
						}
					case "function_call":
						names, args = append(names, o.Name), append(args, o.Args)
					}
				}
			}
			if rec.Code != 200 || strings.Contains(raw, "DSML") || text != "我先搜一下。\n\n" ||
				len(names) != 1 || names[0] != "web_search" || args[0] != wantArgs || strings.Contains(raw, "overloaded") {
				t.Fatalf("streamed %v, first answers %v: %d text %q calls %v %v\n%s", streamed, first, rec.Code, text, names, args, raw)
			}
		}
	}
}

// dsmlResponsesSSE is a Responses stream as DeepSeek's reads: a reasoning
// item, then a message item whose text is text, one character to a delta.
func dsmlResponsesSSE(text string) string {
	ev := func(typ string, v map[string]any) string {
		v["type"] = typ
		b, _ := json.Marshal(v)
		return "event: " + typ + "\ndata: " + string(b)
	}
	seq := 0
	n := func(v map[string]any) map[string]any { v["sequence_number"] = seq; seq++; return v }
	reasoning := map[string]any{"type": "reasoning", "id": "rs_1", "summary": []any{map[string]any{"type": "summary_text", "text": "need a search"}}}
	part := map[string]any{"type": "output_text", "text": text, "annotations": []any{}}
	msg := map[string]any{"type": "message", "role": "assistant", "id": "msg_1", "status": "completed", "content": []any{part}}
	lines := []string{
		ev("response.created", n(map[string]any{"response": map[string]any{"id": "r1", "model": "deepseek-flash", "status": "in_progress", "output": []any{}}})),
		ev("response.output_item.added", n(map[string]any{"output_index": 0, "item": map[string]any{"type": "reasoning", "id": "rs_1", "summary": []any{}}})),
		ev("response.reasoning_summary_text.delta", n(map[string]any{"item_id": "rs_1", "output_index": 0, "summary_index": 0, "delta": "need a search"})),
		ev("response.output_item.done", n(map[string]any{"output_index": 0, "item": reasoning})),
		ev("response.output_item.added", n(map[string]any{"output_index": 1,
			"item": map[string]any{"type": "message", "role": "assistant", "id": "msg_1", "status": "in_progress", "content": []any{}}})),
		ev("response.content_part.added", n(map[string]any{"item_id": "msg_1", "output_index": 1, "content_index": 0,
			"part": map[string]any{"type": "output_text", "text": "", "annotations": []any{}}})),
	}
	for _, r := range text {
		lines = append(lines, ev("response.output_text.delta", n(map[string]any{"item_id": "msg_1", "output_index": 1, "content_index": 0, "delta": string(r)})))
	}
	lines = append(lines,
		ev("response.output_text.done", n(map[string]any{"item_id": "msg_1", "output_index": 1, "content_index": 0, "text": text})),
		ev("response.content_part.done", n(map[string]any{"item_id": "msg_1", "output_index": 1, "content_index": 0, "part": part})),
		ev("response.output_item.done", n(map[string]any{"output_index": 1, "item": msg})),
		ev("response.completed", n(map[string]any{"response": map[string]any{"id": "r1", "model": "deepseek-flash", "status": "completed",
			"output": []any{reasoning, msg}, "usage": map[string]any{"input_tokens": 900, "output_tokens": 120, "total_tokens": 1020}}})))
	return sse(lines...)
}

// The same from Alma to a plugin's DeepSeek (Trae CN, WorkBuddy), alone or
// as a group's member: the plugin's reply, Responses or Chat, whichever the
// plugin was asked, is read the same way.
func TestDSMLAlmaResponsesPlugin(t *testing.T) {
	text := "我先搜一下。\n\n" + observedPipes
	var paths []string
	pid := besideFake(t, "trae-cn", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		w.Header().Set("Content-Type", "text/event-stream")
		if strings.HasSuffix(r.URL.Path, "/responses") {
			io.WriteString(w, dsmlResponsesSSE(text))
			return
		}
		for _, r := range text {
			b, _ := json.Marshal(map[string]any{"id": "c1", "object": "chat.completion.chunk", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": string(r)}, "finish_reason": nil}}})
			io.WriteString(w, "data: "+string(b)+"\n\n")
		}
		io.WriteString(w, `data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	if err := provider.SaveGroup(provider.Group{ID: "ds", Name: "DeepSeek", Members: []string{pid + "/fake-1"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	for _, model := range []string{pid + "/fake-1", "group/ds"} {
		req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"`+model+`","stream":true,"input":[{"role":"user","content":[{"type":"input_text","text":"What is the latest Python?"}]}],`+
			`"tools":[{"type":"function","name":"web_search","parameters":{"type":"object","properties":{"query":{"type":"string"}}},"strict":false}],"tool_choice":"auto"}`))
		req.Header.Set("Authorization", "Bearer magpie")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		raw := rec.Body.String()
		got, names, args, _, _ := responsesStreamRead(t, raw)
		if rec.Code != 200 || strings.Contains(raw, "DSML") || got != "我先搜一下。\n\n" || len(names) != 1 || names[0] != "web_search" ||
			args[0] != `{"query":"current Python version latest stable release"}` {
			t.Fatalf("%s (upstream asked at %v): %d text %q calls %v %v\n%s", model, paths, rec.Code, got, names, args, raw)
		}
	}
}
