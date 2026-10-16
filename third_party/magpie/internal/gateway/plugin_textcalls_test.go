package gateway

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// A plugin's model that writes its tool call into the text is answered as
// a built-in's is (#823, Dazzle-sys: Trae CN's DeepSeek V4.1 Flash in a
// group, in Pi): the plugin's text goes through the same reading, so the
// agent gets the call, whether it speaks Chat or Anthropic Messages.
func TestPluginTextWrittenCall(t *testing.T) {
	// as the screenshot had it: Hermes' block, closed with DeepSeek's tags
	text := "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"cd /e/Project && echo \\\"### $spec\\\"; done\"}}</｜DSML｜parameter>\n</｜DSML｜invoke"
	pid := besideFake(t, "trae-cn", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		for _, part := range []string{text[:40], text[40:]} {
			b, _ := json.Marshal(map[string]any{"id": "c1", "object": "chat.completion.chunk", "choices": []any{map[string]any{"index": 0, "delta": map[string]any{"content": part}, "finish_reason": nil}}})
			io.WriteString(w, "data: "+string(b)+"\n\n")
		}
		io.WriteString(w, `data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	h := New().Handler()
	model := pid + "/fake-1"
	tool := `{"type":"function","function":{"name":"bash","parameters":{"type":"object","properties":{"command":{"type":"string"}}}}}`

	t.Run("chat", func(t *testing.T) {
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+model+`","stream":true,"tools":[`+tool+`],"messages":[{"role":"user","content":"go on"}]}`))
		req.Header.Set("Authorization", "Bearer magpie")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		body := rec.Body.String()
		if rec.Code != 200 || strings.Contains(body, "DSML") || strings.Contains(body, "tool_call>") ||
			!strings.Contains(body, `"tool_calls"`) || !strings.Contains(body, `"finish_reason":"tool_calls"`) {
			t.Fatalf("%d %s", rec.Code, body)
		}
		var args string
		for _, line := range strings.Split(body, "\n") {
			var c struct {
				Choices []struct {
					Delta struct {
						ToolCalls []struct {
							Function struct{ Name, Arguments string }
						} `json:"tool_calls"`
					}
				}
			}
			if json.Unmarshal([]byte(strings.TrimPrefix(line, "data: ")), &c) == nil && len(c.Choices) > 0 {
				for _, tc := range c.Choices[0].Delta.ToolCalls {
					args += tc.Function.Arguments
				}
			}
		}
		var a map[string]string
		if json.Unmarshal([]byte(args), &a) != nil || a["command"] != `cd /e/Project && echo "### $spec"; done` {
			t.Fatalf("arguments %q", args)
		}
	})
	t.Run("chat whole", func(t *testing.T) {
		// asked for one body (#1845): a plugin is asked streamed and the
		// reply given whole, its text read the same way
		req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"`+model+`","stream":false,"tools":[`+tool+`],"messages":[{"role":"user","content":"go on"}]}`))
		req.Header.Set("Authorization", "Bearer magpie")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		body := rec.Body.String()
		var out struct {
			Choices []struct {
				Message struct {
					ToolCalls []struct {
						Function struct{ Name, Arguments string }
					} `json:"tool_calls"`
				}
				FinishReason string `json:"finish_reason"`
			}
		}
		if rec.Code != 200 || json.Unmarshal([]byte(body), &out) != nil || len(out.Choices) != 1 || strings.Contains(body, "DSML") || strings.Contains(body, "tool_call>") ||
			len(out.Choices[0].Message.ToolCalls) != 1 || out.Choices[0].FinishReason != "tool_calls" {
			t.Fatalf("%d %s", rec.Code, body)
		}
		var a map[string]string
		if json.Unmarshal([]byte(out.Choices[0].Message.ToolCalls[0].Function.Arguments), &a) != nil || a["command"] != `cd /e/Project && echo "### $spec"; done` {
			t.Fatalf("arguments %s", body)
		}
	})
	t.Run("messages", func(t *testing.T) {
		req := httptest.NewRequest("POST", "/v1/messages", bytes.NewReader([]byte(`{"model":"`+model+`","stream":true,"max_tokens":100,"tools":[{"name":"bash","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"go on"}]}`)))
		req.Header.Set("x-api-key", "magpie")
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		body := rec.Body.String()
		if rec.Code != 200 || strings.Contains(body, "DSML") || !strings.Contains(body, `"tool_use"`) {
			t.Fatalf("%d %s", rec.Code, body)
		}
	})
}
