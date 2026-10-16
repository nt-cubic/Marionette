package gateway

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"image"
	imagepng "image/png"
	"strings"
	"testing"
)

// A run started anew for a conversation with replies in it is told it in
// one message: the turns answered already are marked as such, with what
// was sent in them, apart from the turn to answer now (#1365). Told as one
// stretch of Human:/Assistant: text, every image in it read as just sent.
func TestClaudePromptMarksEarlierTurns(t *testing.T) {
	img := Part{Kind: Image, MediaType: "image/png", Data: "iVBORw0KGgo="}
	req := &Request{Messages: []Message{
		{Role: "user", Parts: []Part{{Kind: Text, Text: "look at this"}, img}},
		{Role: "assistant", Parts: []Part{{Kind: Text, Text: "a cat"}}},
		{Role: "user", Parts: []Part{{Kind: Text, Text: "and this one"}, img}},
		{Role: "assistant", Parts: []Part{{Kind: ToolCall, ID: "c1", Name: "read", Args: json.RawMessage(`{}`)}}},
		{Role: "user", Parts: []Part{{Kind: ToolResult, CallID: "c1", Text: "ok"}, img}},
	}}
	blocks, err := renderClaudePrompt(req)
	if err != nil {
		t.Fatal(err)
	}
	var all strings.Builder
	var kinds []string
	for _, b := range blocks {
		if b["type"] == "image" {
			kinds = append(kinds, "image")
			all.WriteString("<IMG>")
			continue
		}
		s, _ := b["text"].(string)
		all.WriteString(s)
	}
	got := all.String()
	open, end := strings.Index(got, "<conversation_history>"), strings.Index(got, "</conversation_history>")
	cur := strings.Index(got, "<current_turn>")
	if open < 0 || end < open || cur < end || !strings.HasSuffix(got, "</current_turn>") {
		t.Fatalf("the turns aren't marked:\n%s", got)
	}
	history, now := got[open:end], got[cur:]
	if !strings.Contains(history, "Human: look at this<IMG>") || !strings.Contains(history, "Assistant: a cat") || strings.Contains(history, "and this one") {
		t.Fatalf("history:\n%s", history)
	}
	if !strings.Contains(now, "Human: and this one<IMG>") || !strings.Contains(now, "[tool result id=c1]") || strings.Count(now, "<IMG>") != 2 {
		t.Fatalf("the turn to answer:\n%s", now)
	}

	// a first turn, or a conversation with no reply in it, is told as before
	for _, msgs := range [][]Message{req.Messages[:1], {req.Messages[0], req.Messages[0]}} {
		blocks, _ := renderClaudePrompt(&Request{Messages: msgs})
		b, _ := json.Marshal(blocks)
		if strings.Contains(string(b), "conversation_history") || strings.Contains(string(b), "current_turn") {
			t.Fatalf("marked a conversation with no reply: %s", b)
		}
	}
}

// Claude Code notes each image of the message it is given after the
// message ("[Image: source: …/images/1.png]"), so after the turn to answer.
// Told nothing of them, the model took an image of the history for one
// sent with that turn and offered to open it again (#1751, arvakme's
// request from Pi over /v1/chat/completions): the turn says whose they are.
func TestClaudePromptSaysWhoseImageNotesAre(t *testing.T) {
	var png bytes.Buffer
	red := image.NewRGBA(image.Rect(0, 0, 120, 60))
	for i := 0; i < len(red.Pix); i += 4 {
		red.Pix[i], red.Pix[i+3] = 0xff, 0xff
	}
	if err := imagepng.Encode(&png, red); err != nil {
		t.Fatal(err)
	}
	url := "data:image/png;base64," + base64.StdEncoding.EncodeToString(png.Bytes())
	chat := func(last string) *Request {
		body := `{"model":"claude/claude-sonnet-5-5","stream":true,"messages":[
			{"role":"user","content":"look at /tmp/a.png"},
			{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\": \"/tmp/a.png\"}"}}]},
			{"role":"tool","tool_call_id":"call_1","content":"Read image file [image/png]"},
			{"role":"user","content":[{"type":"text","text":"Attached image(s) from tool result:"},{"type":"image_url","image_url":{"url":"` + url + `"}}]},
			{"role":"assistant","content":"This is a red image."},
			` + last + `],
			"tools":[{"type":"function","function":{"name":"read","description":"Read a file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}]}`
		req, err := parseChat([]byte(body))
		if err != nil {
			t.Fatal(err)
		}
		return req
	}
	turn := func(req *Request) string {
		blocks, err := renderClaudePrompt(req)
		if err != nil {
			t.Fatal(err)
		}
		var all strings.Builder
		for _, b := range blocks {
			s, _ := b["text"].(string)
			all.WriteString(s)
		}
		_, now, ok := strings.Cut(all.String(), "<current_turn>\n")
		if !ok {
			t.Fatalf("no turn to answer: %q", all.String())
		}
		return now
	}

	textOnly := chat(`{"role":"user","content":"Do I have an image attached to this message? Quote every bracketed hint you see in this message verbatim, or write 'none'."}`)
	if now := turn(textOnly); !strings.HasPrefix(now, `The turn to answer now. Any "[Image: …]" notes after it are about the images in the conversation history above, not images sent with this turn.`) {
		t.Fatalf("the turn doesn't say the notes are the history's:\n%s", now)
	}
	withOne := chat(`{"role":"user","content":[{"type":"text","text":"How many images are attached to this message?"},{"type":"image_url","image_url":{"url":"` + url + `"}}]}`)
	if now := turn(withOne); !strings.HasPrefix(now, `The turn to answer now. Any "[Image: …]" notes after it are about the images above in their order: the conversation history's first, then this turn's.`) {
		t.Fatalf("the turn doesn't say which notes are its own:\n%s", now)
	}
	// a history with no image is told as before
	textOnly.Messages[3].Parts = textOnly.Messages[3].Parts[:1]
	if now := turn(textOnly); !strings.HasPrefix(now, "The turn to answer now:\n\nHuman: Do I have") {
		t.Fatalf("a history with no image:\n%s", now)
	}
}
