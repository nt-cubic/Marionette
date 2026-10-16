package gateway

import (
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// codexViewImage is a Codex turn whose view_image call came back with an
// image, as codex-rs 0.162 sends it (FunctionCallOutputContentItem in
// protocol/src/models.rs; view_image.rs answers with the image alone, its
// detail "high"): the function_call_output's output is a list holding one
// input_image, a data: URL (#1861).
const codexViewImage = `{"model":"m","stream":true,"input":[
{"type":"message","role":"user","content":[{"type":"input_text","text":"使用computer use截图当前屏幕看下"}]},
{"type":"function_call","name":"view_image","arguments":"{\"path\":\"/tmp/magpie_icon.png\"}","call_id":"call_view1"},
{"type":"function_call_output","call_id":"call_view1","output":[{"type":"input_image","image_url":"data:image/png;base64,` + viewedPNG + `","detail":"high"}]}
]}`

// a 1x1 PNG
const viewedPNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="

func codexViewImageRequest(t *testing.T) *Request {
	t.Helper()
	r, err := parse(provider.Responses, []byte(codexViewImage))
	if err != nil {
		t.Fatal(err)
	}
	return r
}

// What a plugin on Anthropic's Messages (Kiro's) is handed: the image in
// the tool_result, as an image block of base64 bytes. Kiro's plugin reads it
// there (magpie-community/plugins kiro 0.1.9).
func TestCodexToolImageReachesAnthropicPlugin(t *testing.T) {
	var body struct {
		Messages []struct {
			Content []struct {
				Type      string `json:"type"`
				ToolUseID string `json:"tool_use_id"`
				Content   []struct {
					Type string `json:"type"`
				} `json:"content"`
			} `json:"content"`
		} `json:"messages"`
	}
	b := buildAnthropic(codexViewImageRequest(t), "claude-sonnet-4.5")
	if err := json.Unmarshal(b, &body); err != nil {
		t.Fatal(err)
	}
	last := body.Messages[len(body.Messages)-1].Content
	if len(last) != 1 || last[0].Type != "tool_result" || len(last[0].Content) != 1 || last[0].Content[0].Type != "image" {
		t.Fatalf("tool_result = %s", b)
	}
	if !strings.Contains(string(b), `"source":{"type":"base64","media_type":"image/png","data":"`+viewedPNG+`"}`) {
		t.Fatalf("the image's bytes aren't in the tool_result: %s", b)
	}
}

// Kiro's built-in sends the image with the message answering the call, as
// #369 made it for Claude Code's.
func TestCodexToolImageKiro(t *testing.T) {
	b := buildKiroBody(t, codexViewImageRequest(t), false)
	cur := b.ConversationState.CurrentMessage.UserInputMessage
	if len(cur.Images) != 1 || cur.Images[0].Format != "png" || cur.Images[0].Source.Bytes != viewedPNG {
		t.Fatalf("images = %+v", cur.Images)
	}
	if rs := cur.Context.ToolResults; len(rs) != 1 || rs[0].ToolUseID != "call_view1" {
		t.Fatalf("results = %+v", rs)
	}
}

// Cursor's built-in shows the model a tool's image in the user message
// after the results, as magpie's Chat request gives it Cursor's plugin:
// a result holds text alone, so it says the image follows.
func TestCodexToolImageCursor(t *testing.T) {
	r := codexViewImageRequest(t)
	msgs := cursorMessages(r, bridgeTools(r))
	var tool, after map[string]any
	for i, m := range msgs {
		var x map[string]any
		if err := json.Unmarshal(m, &x); err != nil {
			t.Fatal(err)
		}
		if x["role"] == "tool" && i+1 < len(msgs) {
			tool = x
			json.Unmarshal(msgs[i+1], &after)
		}
	}
	if tool == nil || after == nil || after["role"] != "user" {
		t.Fatalf("no user message after the tool result: %s", msgs)
	}
	res, _ := json.Marshal(tool["content"])
	if !strings.Contains(string(res), "[The tool returned an image; it follows in the next message.]") {
		t.Fatalf("the result doesn't say an image follows: %s", res)
	}
	raw, _ := base64.StdEncoding.DecodeString(viewedPNG)
	content, _ := json.Marshal(after["content"])
	if !strings.Contains(string(content), `"hex":"`+hex.EncodeToString(raw)+`"`) || !strings.Contains(string(content), `"mimeType":"image/png"`) ||
		!strings.Contains(string(content), "[From the result of view_image (tool call call_view1):]") {
		t.Fatalf("the image isn't in the message after: %s", content)
	}
}
