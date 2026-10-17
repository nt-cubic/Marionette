package gateway

import (
	"bytes"
	"encoding/json"
	"slices"
	"strings"
)

// chatTidy mends a Chat Completions stream relayed as it is. WorkBuddy's
// (copilot.tencent.com) keeps sending "reasoning_content": "" in every
// delta once the model has done thinking; its own CLI ignores an empty one
// after the thinking is over, Cline does too, but Qoder takes each for
// the thinking starting again and each piece of text after it for a new
// message, so a reply read one word to a line. An empty reasoning_content
// says nothing, so it is left out.
//
// Two more of the stream's shapes are mended with it. Every chunk that
// hasn't finished carries "finish_reason": "" where the OpenAI contract
// has null; an empty string is not one of the field's values and a strict
// client (the AI SDK) rejects the chunk for it, so it becomes null. And
// every tool-call argument fragment after the first repeats
// "function": {"name": ""} although the real name went with the first
// fragment; a client that merges a delta by overwriting any field it
// carries (the grok CLI) assembles a tool call named "" and fails it with
// "Tool not found", so the empty duplicate is dropped and the real name
// is only sent once.
//
// And every delta carries "tool_calls": [], text and thinking too. An
// empty list says nothing, but Qoder (qodercli 1.1.65's stream adapter
// tests delta.tool_calls for being there, not for holding any) closes its
// text block on each one, so each piece of text after it opened a block
// of its own and a reply read a word to a line ("我来 / 看 / 一下 / …");
// the empty list is left out.
//
// And Mistral sends a reasoning model's thinking (GLM on api.mistral.ai)
// as typed parts: "content": [{"type": "thinking", "thinking": [{"type":
// "text", "text": "…"}]}, {"type": "text", "text": "ok"}], thinking and
// text sometimes in one delta. The contract has a string or null there,
// and the AI SDK OpenCode reads Chat with turns the first such chunk away
// ("Invalid … stream event", #483), so the thinking goes to
// reasoning_content, as other upstreams send it, and the text to a string
// content.
//
// And an upstream that sometimes sends a tool call's first fragment with
// no id (OpenCode Zen's free DeepSeek) has the agent keep the call as "",
// which a strict upstream it is sent to later turns away (#2611): such a
// fragment is given an id, and a later fragment's empty one is left out.
//
// Lines pass whole, as they came, unless one has any of them.
type chatTidy struct {
	buf []byte
	ids streamCallIDs
}

var (
	emptyReasoning    = []byte(`"reasoning_content":""`)
	emptyFinishReason = []byte(`"finish_reason":""`)
	emptyToolName     = []byte(`"name":""`)
	emptyToolCalls    = []byte(`"tool_calls":[]`)
	contentParts      = []byte(`"content":[`)
	contentPartsSpace = []byte(`"content": [`)
	toolCallsField    = []byte(`"tool_calls"`)
)

// write takes what was read and gives back what to send on: every line
// that is complete, mended.
func (t *chatTidy) write(b []byte) []byte {
	t.buf = append(t.buf, b...)
	i := bytes.LastIndexByte(t.buf, '\n')
	if i < 0 {
		return nil
	}
	out := t.lines(t.buf[:i+1])
	t.buf = append(t.buf[:0], t.buf[i+1:]...)
	return out
}

// flush is what's left once the stream ends.
func (t *chatTidy) flush() []byte {
	out := t.lines(t.buf)
	t.buf = nil
	return out
}

func (t *chatTidy) lines(b []byte) []byte {
	if !needsTidy(b) {
		return append([]byte(nil), b...)
	}
	var out []byte
	for len(b) > 0 {
		line := b
		if i := bytes.IndexByte(b, '\n'); i >= 0 {
			line, b = b[:i+1], b[i+1:]
		} else {
			b = nil
		}
		out = append(out, t.line(line)...)
	}
	return out
}

// needsTidy reports whether a line carries any of the shapes chatTidy
// mends.
func needsTidy(line []byte) bool {
	return bytes.Contains(line, emptyReasoning) ||
		bytes.Contains(line, emptyFinishReason) ||
		bytes.Contains(line, emptyToolName) ||
		bytes.Contains(line, emptyToolCalls) ||
		bytes.Contains(line, contentParts) ||
		bytes.Contains(line, contentPartsSpace) ||
		bytes.Contains(line, toolCallsField)
}

// tidyLine mends one line as the first of a stream.
func tidyLine(line []byte) []byte { return (&chatTidy{}).line(line) }

func (t *chatTidy) line(line []byte) []byte {
	if !needsTidy(line) {
		return line
	}
	body := bytes.TrimRight(line, "\r\n")
	end := line[len(body):]
	data, ok := bytes.CutPrefix(body, []byte("data:"))
	if !ok {
		return line
	}
	var chunk map[string]json.RawMessage
	if decodeJSON(data, &chunk) != nil {
		return line
	}
	var choices []map[string]json.RawMessage
	if decodeJSON(chunk["choices"], &choices) != nil {
		return line
	}
	changed := false
	for k, c := range choices {
		if string(c["finish_reason"]) == `""` {
			c["finish_reason"] = json.RawMessage("null")
			changed = true
		}
		var delta map[string]json.RawMessage
		if decodeJSON(c["delta"], &delta) != nil {
			continue
		}
		inDelta := flattenContent(delta, false)
		if string(delta["reasoning_content"]) == `""` {
			delete(delta, "reasoning_content")
			inDelta = true
		}
		if calls, ok := delta["tool_calls"]; ok && string(calls) == "[]" {
			delete(delta, "tool_calls")
			inDelta = true
		} else if ok {
			if t.ids == nil {
				t.ids = streamCallIDs{}
			}
			choice := k
			decodeJSON(c["index"], &choice)
			if mended, chg := t.ids.mend(choice, calls); chg {
				calls, delta["tool_calls"] = mended, mended
				inDelta = true
			}
			if mended, chg := withoutEmptyNames(calls); chg {
				delta["tool_calls"] = mended
				inDelta = true
			}
		}
		if inDelta {
			c["delta"], _ = json.Marshal(delta)
			changed = true
		}
	}
	if !changed {
		return line
	}
	chunk["choices"], _ = json.Marshal(choices)
	nb, err := json.Marshal(chunk)
	if err != nil {
		return line
	}
	return append(append([]byte("data: "), nb...), end...)
}

// withoutEmptyNames drops the empty "name" on a delta's tool-call
// fragments and reports whether any was dropped. A fragment whose
// function carries no name at all is left as it is; one with a real name
// keeps it.
func withoutEmptyNames(raw json.RawMessage) (json.RawMessage, bool) {
	var calls []map[string]json.RawMessage
	if decodeJSON(raw, &calls) != nil {
		return raw, false
	}
	changed := false
	for _, call := range calls {
		var fn map[string]json.RawMessage
		if decodeJSON(call["function"], &fn) != nil {
			continue
		}
		if string(fn["name"]) != `""` {
			continue
		}
		delete(fn, "name")
		call["function"], _ = json.Marshal(fn)
		changed = true
	}
	if !changed {
		return raw, false
	}
	out, err := json.Marshal(calls)
	if err != nil {
		return raw, false
	}
	return out, true
}

// partsText reads a content that is an array of typed parts: its text,
// and the text of its thinking parts. ok is false for any other content,
// a string or null.
func partsText(raw json.RawMessage) (text, think string, ok bool) {
	raw = bytes.TrimSpace(raw)
	if len(raw) == 0 || raw[0] != '[' {
		return "", "", false
	}
	var parts []struct {
		Type     string          `json:"type"`
		Text     string          `json:"text"`
		Thinking json.RawMessage `json:"thinking"`
	}
	if decodeJSON(raw, &parts) != nil {
		return "", "", false
	}
	var tb, kb strings.Builder
	for _, p := range parts {
		switch p.Type {
		case "text":
			tb.WriteString(p.Text)
		case "thinking":
			// Mistral's is a list of text parts; a plain string is taken too
			var s string
			if decodeJSON(p.Thinking, &s) == nil {
				kb.WriteString(s)
				continue
			}
			var inner []struct {
				Text string `json:"text"`
			}
			decodeJSON(p.Thinking, &inner)
			for _, in := range inner {
				kb.WriteString(in.Text)
			}
		}
	}
	return tb.String(), kb.String(), true
}

// flattenContent turns a delta's or message's content of typed parts into
// a string content, and its thinking into reasoning_content after any
// already there, and reports whether it did. A delta with no text left
// goes without content; a whole message keeps an empty one.
func flattenContent(m map[string]json.RawMessage, whole bool) bool {
	text, think, ok := partsText(m["content"])
	if !ok {
		return false
	}
	if text != "" || whole {
		m["content"], _ = json.Marshal(text)
	} else {
		delete(m, "content")
	}
	if think != "" {
		var had string
		decodeJSON(m["reasoning_content"], &had)
		m["reasoning_content"], _ = json.Marshal(had + think)
	}
	return true
}

// chatWhole mends a Chat Completions reply that came whole, not streamed:
// a message whose content is typed parts (Mistral's thinking, #483) goes
// on with a string content and its thinking in reasoning_content, and,
// when the request offered tools (names), calls the model wrote into its
// text are made the calls they are, as textCallTidy makes them in a
// stream (#1845), and a call the upstream gave no id is given one, as
// chatTidy gives it in a stream (#2611). It is held until it ends; any
// other reply passes byte for byte.
type chatWhole struct {
	names map[string]bool
	buf   []byte
}

func (t *chatWhole) write(b []byte) []byte {
	t.buf = append(t.buf, b...)
	return nil
}

func (t *chatWhole) flush() []byte {
	b := t.buf
	t.buf = nil
	parts := bytes.Contains(b, contentParts) || bytes.Contains(b, contentPartsSpace)
	if !parts && !mayWriteCalls(b, t.names) && !bytes.Contains(b, toolCallsField) {
		return b
	}
	var reply map[string]json.RawMessage
	if decodeJSON(b, &reply) != nil {
		return b
	}
	var choices []map[string]json.RawMessage
	if decodeJSON(reply["choices"], &choices) != nil {
		return b
	}
	changed := false
	for _, c := range choices {
		var msg map[string]json.RawMessage
		if decodeJSON(c["message"], &msg) != nil {
			continue
		}
		mended := parts && flattenContent(msg, true)
		// a call the upstream gave no id (#2611)
		mended = wholeCallIDs(msg) || mended
		if wrote, called := wholeTextCalls(msg, t.names); wrote {
			if called {
				c["finish_reason"] = json.RawMessage(`"tool_calls"`)
			}
			mended = true
		}
		if !mended {
			continue
		}
		c["message"], _ = marshalPlain(msg)
		changed = true
	}
	if !changed {
		return b
	}
	reply["choices"], _ = marshalPlain(choices)
	nb, err := marshalPlain(reply)
	if err != nil {
		return b
	}
	return nb
}

// messageReasoning are the fields a Chat client sends an earlier
// assistant turn's thinking back in: reasoning_content (DeepSeek's, which
// OpenCode and most clients send), reasoning (OpenRouter's) and
// reasoning_details (OpenRouter's structured one). DeepSeek, Kimi and the
// like want it back; Mistral turns each away as extra_forbidden (#494).
var messageReasoning = []string{"reasoning_content", "reasoning", "reasoning_details"}

// reasoningText is the thinking a message carries in messageReasoning:
// reasoning_content, else reasoning, else reasoning_details' text.
func reasoningText(m map[string]json.RawMessage) string {
	for _, f := range messageReasoning[:2] {
		var s string
		if decodeJSON(m[f], &s) == nil && s != "" {
			return s
		}
	}
	var details []struct {
		Text    string `json:"text"`
		Summary string `json:"summary"`
	}
	decodeJSON(m["reasoning_details"], &details)
	var b strings.Builder
	for _, d := range details {
		if d.Text != "" {
			b.WriteString(d.Text)
		} else {
			b.WriteString(d.Summary)
		}
	}
	return b.String()
}

// noReasoning is the reasoning_content a tool call goes to an upstream
// that wants it back (chatReplaysReasoning) with, when the client has none
// for it: a turn another model made, or one whose thinking the client
// didn't keep. DeepSeek turns the whole request away when a tool call of
// the turn being continued has none ("The `reasoning_content` in the
// thinking mode must be passed back to the API", #1462). Its Chat API
// takes an empty one, but a relay that leaves empty fields out would send
// none, and its Responses API turns an empty reasoning_text away, so it is
// a space.
const noReasoning = " "

// withReasoningOnToolTurns gives each assistant message of a Chat request
// that has tool calls, or that follows a tool message of its turn, and no
// reasoning_content one: the thinking it sent back in another field
// (reasoning, reasoning_details), else noReasoning. DeepSeek turns away a
// reply after a tool result with none as it does a call (checked against
// api.deepseek.com on 2026-10-12, #1822). A message with reasoning_content
// of its own keeps it, any other reply is left as it is (DeepSeek doesn't
// ask for its thinking), and a body with no tool calls comes back byte for
// byte.
func withReasoningOnToolTurns(body []byte) []byte {
	afterTool := false
	return editMessages(body, []string{"tool_calls"}, func(m map[string]json.RawMessage) bool {
		var role string
		var calls []json.RawMessage
		decodeJSON(m["role"], &role)
		switch role {
		case "tool":
			afterTool = true
		case "user":
			afterTool = false
		}
		if raw, ok := m["tool_calls"]; ok && decodeJSON(raw, &calls) != nil {
			return false
		}
		if role != "assistant" || len(calls) == 0 && !afterTool {
			return false
		}
		var own *string
		if decodeJSON(m["reasoning_content"], &own) == nil && own != nil {
			return false
		}
		think := reasoningText(m)
		if think == "" {
			think = noReasoning
		}
		m["reasoning_content"], _ = marshalPlain(think)
		return true
	})
}

// silentAssistantsOut is withoutSilentAssistants for a Chat request
// relayed as the client sent it (#2599): an agent that speaks Chat keeps
// the turn it cut off as {"role":"assistant","content":""} (or null, with
// reasoning_content alone) and sends it again on every later turn. A body
// without such a turn comes back byte for byte.
func silentAssistantsOut(body []byte) []byte {
	if !bytes.Contains(body, []byte(`"assistant"`)) {
		return body
	}
	var q map[string]json.RawMessage
	if decodeJSON(body, &q) != nil {
		return body
	}
	dec := json.NewDecoder(bytes.NewReader(q["messages"]))
	dec.UseNumber()
	var msgs []map[string]any
	if dec.Decode(&msgs) != nil {
		return body
	}
	msgs, changed := withoutSilentAssistants(msgs)
	if !changed {
		return body
	}
	q["messages"], _ = marshalPlain(msgs)
	nb, err := marshalPlain(q)
	if err != nil {
		return body
	}
	return nb
}

// reasoningDetailsAsContent leaves reasoning_details out of every message
// of a Chat request, its thinking kept in reasoning_content when the
// message has none: Kimi's API turns reasoning_details away in whatever
// shape it comes, OpenRouter's reasoning.text with a stream's index in it
// (as another vendor's turn has it) or without ("the reasoning_details at
// position 2 entry 0 must not contain streaming index", "… has an invalid
// type", #1765), and takes reasoning_content.
func reasoningDetailsAsContent(body []byte) []byte {
	return editMessages(body, []string{"reasoning_details"}, func(m map[string]json.RawMessage) bool {
		if _, ok := m["reasoning_details"]; !ok {
			return false
		}
		var own string
		if decodeJSON(m["reasoning_content"], &own) != nil || own == "" {
			if think := reasoningText(m); think != "" {
				m["reasoning_content"], _ = marshalPlain(think)
			}
		}
		delete(m, "reasoning_details")
		return true
	})
}

// withoutMessageReasoning leaves the thinking fields an upstream refused
// out of every message of a Chat request; reasoning_details' thinking
// stays in reasoning_content unless that was refused too.
func withoutMessageReasoning(body []byte, fields []string) []byte {
	if slices.Contains(fields, "reasoning_details") && !slices.Contains(fields, "reasoning_content") {
		body = reasoningDetailsAsContent(body)
	}
	return withoutMessageFields(body, fields...)
}

// editMessages hands each message of a Chat request whose JSON names any
// of fields to edit, and puts back those edit reports it changed; every
// other message keeps its bytes. The body comes back as it was if nothing
// changed.
func editMessages(body []byte, fields []string, edit func(m map[string]json.RawMessage) bool) []byte {
	has := false
	for _, f := range fields {
		has = has || bytes.Contains(body, []byte(`"`+f+`"`))
	}
	if !has {
		return body
	}
	var q map[string]json.RawMessage
	if decodeJSON(body, &q) != nil {
		return body
	}
	var msgs []json.RawMessage
	if decodeJSON(q["messages"], &msgs) != nil {
		return body
	}
	changed := false
	for i, raw := range msgs {
		var m map[string]json.RawMessage
		if decodeJSON(raw, &m) != nil || !edit(m) {
			continue
		}
		if nb, err := marshalPlain(m); err == nil {
			msgs[i], changed = nb, true
		}
	}
	if !changed {
		return body
	}
	q["messages"], _ = marshalPlain(msgs)
	nb, err := marshalPlain(q)
	if err != nil {
		return body
	}
	return nb
}

// withoutMessageFields leaves fields out of every message of a Chat
// request.
func withoutMessageFields(body []byte, fields ...string) []byte {
	return editMessages(body, fields, func(m map[string]json.RawMessage) bool {
		changed := false
		for _, f := range fields {
			if _, ok := m[f]; ok {
				delete(m, f)
				changed = true
			}
		}
		return changed
	})
}

// mistralThinking puts an assistant turn's thinking, sent back in
// reasoning_content (or reasoning, reasoning_details) as OpenCode does,
// into the typed thinking part Mistral itself returns it in, ahead of the
// turn's content: Mistral turns those fields away with 422 extra_forbidden
// (#494) and takes the part, so the thinking stays in context. A turn
// with tool calls and no text has the thinking part alone for content.
func mistralThinking(body []byte) []byte {
	return editMessages(body, messageReasoning, func(m map[string]json.RawMessage) bool {
		had := false
		for _, f := range messageReasoning {
			_, ok := m[f]
			had = had || ok
		}
		if !had {
			return false
		}
		think := reasoningText(m)
		for _, f := range messageReasoning {
			delete(m, f)
		}
		var role string
		decodeJSON(m["role"], &role)
		if think == "" || role != "assistant" {
			return true
		}
		thinking, _ := marshalPlain(map[string]any{"type": "thinking",
			"thinking": []map[string]string{{"type": "text", "text": think}}})
		parts := []json.RawMessage{thinking}
		c := bytes.TrimSpace(m["content"])
		var list []json.RawMessage
		var s string
		switch {
		case len(c) > 0 && c[0] == '[' && decodeJSON(c, &list) == nil:
			parts = append(parts, list...)
		case decodeJSON(c, &s) == nil && s != "":
			text, _ := marshalPlain(map[string]string{"type": "text", "text": s})
			parts = append(parts, text)
		}
		m["content"], _ = marshalPlain(parts)
		return true
	})
}
