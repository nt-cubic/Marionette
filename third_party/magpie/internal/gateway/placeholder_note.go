package gateway

import (
	"bytes"
	"encoding/json"
	"strconv"
	"strings"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/redact"
)

// placeholderNote tells a model that the placeholders masking put in a
// request stand for real values that are put back before anything runs,
// so it uses them as written: a less able model refused to search with
// one, asked to verify it, or wrote it as "an example name" (treeMan on
// Discord). It goes to the vendor, so it names neither magpie nor any
// vendor or client (LESSONS: text magpie adds to a prompt), and it is the
// same on every turn, so a cached prefix stays cached. Its example isn't
// a placeholder masking writes (a 1 isn't in its alphabet), so it is
// never taken for one or put back.
const placeholderNote = "Values written like {{NAME_ab12cd34}} are placeholders for real values. " +
	"Use them exactly as written, in replies and in tool calls alike: they are filled in with the real values before anything runs. " +
	"Don't ask to verify them, and don't replace them with examples."

// noteBlock is placeholderNote as a block or part after the text it
// follows: a translator that joins a message's blocks into one string
// (Anthropic or Gemini to Chat) keeps it a paragraph of its own.
const noteBlock = "\n\n" + placeholderNote

// notesPlaceholders says whether a request masked with a value swapped
// for a placeholder is told so (placeholderNote) on its way to p. Not on
// two paths, where the request is to look as its own client sent it:
//   - a Claude subscription, run through the Claude Code binary, which
//     puts the caller's instructions in the run's first message, the
//     content Anthropic classifies (claude-subscription-bridge.md);
//   - Codex's ChatGPT sign-in, whose instructions are Codex's own
//     base_instructions and are never changed.
//
// Nor a Responses turn that goes on from a stored response: its input is
// only the new items, and the note, said each turn, would pile up in the
// stored conversation.
func notesPlaceholders(p provider.Provider, from provider.Protocol, body []byte) bool {
	if p.Account != nil && (p.Account.Agent == "claude" || p.Account.Agent == "codex") {
		return false
	}
	if from == provider.Responses && gjson.GetBytes(body, "previous_response_id").String() != "" {
		return false
	}
	return true
}

// withPlaceholderNote is body with placeholderNote said where its
// placeholders first appear, so the conversation before that point goes
// as it went and its cached prefix holds (TestMaskedConversationKeeps-
// ItsCachedPrefix): a value is masked first in new text, a tool's output
// halfway through a task, and the text sent before it is sent again as
// it went. A system prompt or tools with one has it from the first turn,
// and is told in the instructions: the last block of Anthropic's system,
// the end of Chat's first system (or developer) message, a part after
// Gemini's systemInstruction's. Otherwise the earliest message with one
// says it, after what it has: a text block or part of its own, or a line
// at the end of a message that is one string. Responses' instructions are
// never changed (Codex's base_instructions): a developer message of its
// own goes first in its input, or after the earliest item with one and
// the tool outputs beside it. On a later turn the same message is the
// earliest, told the same, so the note never moves. A body it can't read,
// or with no placeholder or no place for it, is returned as it is; the
// edit is made in place in the bytes, the rest as the client wrote it.
func withPlaceholderNote(from provider.Protocol, body []byte) []byte {
	if !gjson.ValidBytes(body) || !gjson.ParseBytes(body).IsObject() || !redact.HasPlaceholder(string(body)) {
		return body
	}
	switch from {
	case provider.Anthropic:
		return withNoteAnthropic(body)
	case provider.Chat:
		return withNoteChat(body)
	case provider.Responses:
		return withNoteResponses(body)
	case provider.Gemini:
		return withNoteGemini(body)
	}
	return body
}

// holds says whether r's text has a placeholder.
func holdsPlaceholder(r gjson.Result) bool { return r.Exists() && redact.HasPlaceholder(r.Raw) }

// earliest is the index of the first element of the array r with a
// placeholder, -1 for none.
func earliest(r gjson.Result) int {
	for i, e := range r.Array() {
		if holdsPlaceholder(e) {
			return i
		}
	}
	return -1
}

func withNoteAnthropic(body []byte) []byte {
	s := gjson.GetBytes(body, "system")
	if holdsPlaceholder(s) || holdsPlaceholder(gjson.GetBytes(body, "tools")) {
		switch {
		case !s.Exists():
			return withTopField(body, "system", noteQuoted(placeholderNote))
		case s.Type == gjson.Null:
			return spliced(body, s, noteQuoted(placeholderNote))
		case s.Type == gjson.String:
			return spliced(body, s, noteQuoted(withLine(s.String(), placeholderNote)))
		case s.IsArray():
			return appended(body, s, textPart("text"))
		}
		return body
	}
	i := earliest(gjson.GetBytes(body, "messages"))
	if i < 0 {
		return body
	}
	c := gjson.GetBytes(body, "messages."+strconv.Itoa(i)+".content")
	switch {
	case c.Type == gjson.String:
		blocks := noteJSON([]any{map[string]any{"type": "text", "text": c.String()}, map[string]any{"type": "text", "text": noteBlock}})
		return spliced(body, c, blocks)
	case c.IsArray():
		return appended(body, c, textPart("text"))
	}
	return body
}

func withNoteChat(body []byte) []byte {
	msgs := gjson.GetBytes(body, "messages")
	if !msgs.IsArray() {
		return body
	}
	i := earliest(msgs)
	if holdsPlaceholder(gjson.GetBytes(body, "tools")) || i >= 0 && isSystemRole(msgs.Array()[i]) {
		// the instructions: the first system message, or one of their own
		i = -1
		for j, m := range msgs.Array() {
			if isSystemRole(m) {
				i = j
				break
			}
		}
		if i < 0 {
			return prepended(body, msgs, noteJSON(map[string]any{"role": "system", "content": placeholderNote}))
		}
	}
	if i < 0 {
		return body
	}
	c := gjson.GetBytes(body, "messages."+strconv.Itoa(i)+".content")
	switch {
	case c.Type == gjson.String:
		return spliced(body, c, noteQuoted(withLine(c.String(), placeholderNote)))
	case c.IsArray():
		return appended(body, c, textPart("text"))
	}
	return body
}

func isSystemRole(m gjson.Result) bool {
	role := m.Get("role").String()
	return role == "system" || role == "developer"
}

func withNoteResponses(body []byte) []byte {
	in := gjson.GetBytes(body, "input")
	dev := noteJSON(map[string]any{"type": "message", "role": "developer", "content": []any{map[string]any{"type": "input_text", "text": placeholderNote}}})
	if in.Type == gjson.String {
		user := noteJSON(map[string]any{"role": "user", "content": in.String()})
		return spliced(body, in, []byte("["+string(dev)+","+string(user)+"]"))
	}
	if !in.IsArray() {
		return body
	}
	if holdsPlaceholder(gjson.GetBytes(body, "instructions")) || holdsPlaceholder(gjson.GetBytes(body, "tools")) {
		return prepended(body, in, dev)
	}
	items := in.Array()
	i := earliest(in)
	if i < 0 {
		return body
	}
	// after the outputs beside it: a call's run of outputs stays whole
	for i+1 < len(items) && strings.HasSuffix(items[i+1].Get("type").String(), "_call_output") {
		i++
	}
	last := gjson.GetBytes(body, "input."+strconv.Itoa(i))
	if !at(body, last) {
		return body
	}
	return insertAt(body, last.Index+len(last.Raw), ","+string(dev))
}

func withNoteGemini(body []byte) []byte {
	for _, key := range []string{"systemInstruction", "system_instruction"} {
		si := gjson.GetBytes(body, key)
		if !si.Exists() || si.Type == gjson.Null {
			continue
		}
		if !holdsPlaceholder(si) && !holdsPlaceholder(gjson.GetBytes(body, "tools")) {
			break
		}
		if parts := gjson.GetBytes(body, key+".parts"); parts.IsArray() {
			return appended(body, parts, noteJSON(map[string]any{"text": noteBlock}))
		}
		return body
	}
	text := noteJSON(map[string]any{"text": noteBlock})
	if holdsPlaceholder(gjson.GetBytes(body, "tools")) {
		return withTopField(body, "systemInstruction", noteJSON(map[string]any{"parts": []any{map[string]any{"text": placeholderNote}}}))
	}
	i := earliest(gjson.GetBytes(body, "contents"))
	if i < 0 {
		return body
	}
	if parts := gjson.GetBytes(body, "contents."+strconv.Itoa(i)+".parts"); parts.IsArray() {
		return appended(body, parts, text)
	}
	return body
}

// textPart is placeholderNote as a content part of the given type.
func textPart(typ string) []byte {
	return noteJSON(map[string]any{"type": typ, "text": noteBlock})
}

// quoted is s as a JSON string, <, > and & as written.
func noteQuoted(s string) []byte { return noteJSON(s) }

func noteJSON(v any) []byte {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(v)
	return bytes.TrimSuffix(b.Bytes(), []byte("\n"))
}

// at says whether r's raw text is where its Index says in body: gjson
// sets no Index for a value it can't place.
func at(body []byte, r gjson.Result) bool {
	return r.Index > 0 && r.Index+len(r.Raw) <= len(body) && string(body[r.Index:r.Index+len(r.Raw)]) == r.Raw
}

// spliced is body with r's value replaced by v.
func spliced(body []byte, r gjson.Result, v []byte) []byte {
	if !at(body, r) {
		return body
	}
	out := make([]byte, 0, len(body)+len(v))
	out = append(out, body[:r.Index]...)
	out = append(out, v...)
	return append(out, body[r.Index+len(r.Raw):]...)
}

// appended is body with item added at the end of the array r.
func appended(body []byte, r gjson.Result, item []byte) []byte {
	if !at(body, r) {
		return body
	}
	end := r.Index + len(r.Raw) - 1 // its ]
	sep := ","
	if len(r.Array()) == 0 {
		sep = ""
	}
	return insertAt(body, end, sep+string(item))
}

// prepended is body with item put first in the array r.
func prepended(body []byte, r gjson.Result, item []byte) []byte {
	if !at(body, r) {
		return body
	}
	sep := ","
	if len(r.Array()) == 0 {
		sep = ""
	}
	return insertAt(body, r.Index+1, string(item)+sep)
}

// withTopField is the object body with a field it doesn't have added
// first.
func withTopField(body []byte, key string, v []byte) []byte {
	i := bytes.IndexByte(body, '{')
	if i < 0 {
		return body
	}
	sep := ","
	if len(gjson.ParseBytes(body).Map()) == 0 {
		sep = ""
	}
	return insertAt(body, i+1, string(noteQuoted(key))+":"+string(v)+sep)
}

func insertAt(body []byte, i int, s string) []byte {
	out := make([]byte, 0, len(body)+len(s))
	out = append(out, body[:i]...)
	out = append(out, s...)
	return append(out, body[i:]...)
}
