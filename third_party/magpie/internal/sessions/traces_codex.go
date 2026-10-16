package sessions

import (
	"encoding/json"
	"fmt"
	"strings"
)

// A Codex model response's bodies come from the rollout's response items, the
// record of what the model was sent and what it returned. Each usage record
// closes one response: its input is the turn's prompt (base instructions,
// developer and user messages) and what came back since the previous
// response (tool results); its output is only what that response produced
// (assistant text and tool calls, with their call IDs). The UI's
// item_completed echoes of the same messages are not part of it.
type codexTranscript struct {
	prompt, fed, out []json.RawMessage
}

type codexChatMessage struct {
	Role       string          `json:"role"`
	Content    json.RawMessage `json:"content,omitempty"`
	ToolCalls  []codexChatCall `json:"tool_calls,omitempty"`
	ToolCallID string          `json:"tool_call_id,omitempty"`
}

type codexChatCall struct {
	ID       string `json:"id"`
	Type     string `json:"type"`
	Function struct {
		Name      string `json:"name"`
		Arguments string `json:"arguments"`
	} `json:"function"`
}

func codexChat(m codexChatMessage) json.RawMessage {
	b, _ := json.Marshal(m)
	return b
}

// codexTraceContent turns Codex's input_text/output_text parts into text parts. An
// inline image (a screenshot is megabytes) is named, not copied, since the
// prompt goes out again with every response of the turn.
func codexTraceContent(raw json.RawMessage) json.RawMessage {
	var text string
	if json.Unmarshal(raw, &text) == nil {
		b, _ := json.Marshal(text)
		return b
	}
	var parts []json.RawMessage
	if json.Unmarshal(raw, &parts) != nil {
		return raw
	}
	for i, part := range parts {
		var p struct {
			Type     string
			Text     string
			ImageURL string `json:"image_url"`
		}
		if json.Unmarshal(part, &p) != nil {
			continue
		}
		switch p.Type {
		case "input_text", "output_text", "text":
			parts[i], _ = json.Marshal(map[string]string{"type": "text", "text": p.Text})
		case "input_image":
			if media, data, ok := strings.Cut(p.ImageURL, ","); ok && strings.HasPrefix(media, "data:") {
				label := fmt.Sprintf("[image %s, %d bytes not exported]", strings.TrimSuffix(strings.TrimPrefix(media, "data:"), ";base64"), len(data))
				parts[i], _ = json.Marshal(map[string]string{"type": "text", "text": label})
			}
		}
	}
	b, _ := json.Marshal(parts)
	return b
}

// codexArguments gives a call's arguments as the string the model wrote:
// function_call arguments are a JSON string, custom_tool_call input free text.
func codexArguments(raw json.RawMessage) string {
	var s string
	if json.Unmarshal(raw, &s) == nil {
		return s
	}
	return string(raw)
}

func (x *codexTranscript) message(role string, content json.RawMessage) {
	m := codexChat(codexChatMessage{Role: role, Content: codexTraceContent(content)})
	if role == "assistant" {
		x.out = append(x.out, m)
		return
	}
	x.prompt = append(x.prompt, m)
}

func (x *codexTranscript) call(id, name string, arguments json.RawMessage) {
	call := codexChatCall{ID: id, Type: "function"}
	call.Function.Name, call.Function.Arguments = name, codexArguments(arguments)
	x.out = append(x.out, codexChat(codexChatMessage{Role: "assistant", ToolCalls: []codexChatCall{call}}))
}

func (x *codexTranscript) result(id string, output json.RawMessage) {
	x.fed = append(x.fed, codexChat(codexChatMessage{Role: "tool", ToolCallID: id, Content: codexTraceContent(output)}))
}

// response closes one model response. ok is false when the turn recorded no
// response items, so the caller keeps its older fallback.
func (x *codexTranscript) response(base string) (input, output string, ok bool) {
	if len(x.prompt) == 0 && len(x.fed) == 0 && len(x.out) == 0 {
		return "", "", false
	}
	var in []json.RawMessage
	if base != "" {
		content, _ := json.Marshal(base)
		in = append(in, codexChat(codexChatMessage{Role: "system", Content: content}))
	}
	in = append(append(in, x.prompt...), x.fed...)
	b, _ := json.Marshal(in)
	input = string(b)
	if len(x.out) > 0 {
		b, _ = json.Marshal(x.out)
		output = string(b)
	}
	x.fed, x.out = nil, nil
	return input, output, true
}

// codexNativeBodies fills what a native operation recorded besides command
// output: a file change's diff, the image viewed, an extension's query and
// results. An image generation's result is the image itself (megabytes of
// base64), so only where it was saved is kept.
func codexNativeBodies(s *TraceSpan, typ, path string, changes json.RawMessage, stdout, stderr string, query, action json.RawMessage, prompt string, results json.RawMessage, saved string, failure json.RawMessage) {
	switch typ {
	case "FileChange":
		if len(changes) > 0 {
			s.Input = string(changes)
		}
		if s.Output == "" && (stdout != "" || stderr != "") {
			b, _ := json.Marshal(map[string]string{"stdout": stdout, "stderr": stderr})
			s.Output = string(b)
		}
	case "ImageView":
		if path != "" {
			b, _ := json.Marshal(map[string]string{"path": path})
			s.Input = string(b)
		}
	case "Extension":
		in := map[string]any{}
		if len(query) > 0 && string(query) != "null" {
			in["query"] = query
		}
		if len(action) > 0 && string(action) != "null" {
			in["action"] = action
		}
		if prompt != "" {
			in["prompt"] = prompt
		}
		if len(in) > 0 {
			b, _ := json.Marshal(in)
			s.Input = string(b)
		}
		out := map[string]any{}
		if len(results) > 0 && string(results) != "null" {
			out["results"] = results
		}
		if saved != "" {
			out["saved_path"] = saved
		}
		if len(failure) > 0 && string(failure) != "null" {
			out["failure"] = failure
		}
		if len(out) > 0 {
			b, _ := json.Marshal(out)
			s.Output = string(b)
		}
	}
}
