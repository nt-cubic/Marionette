package gateway

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// deepseekRefuseAnthropic answers as api.deepseek.com/anthropic does
// (deepseek-flash and deepseek-v4-pro, checked 2026-10-11 for #1611): a
// redacted_thinking block anywhere is a body it can't read (422), and,
// thinking (no thinking field, enabled or adaptive), an assistant message
// with a tool_use and no thinking block turns the whole request away (400).
// Thinking disabled, a bare tool_use is taken.
func deepseekRefuseAnthropic(body []byte) (int, string) {
	var q struct {
		Thinking *struct {
			Type string `json:"type"`
		} `json:"thinking"`
		Messages []struct {
			Role    string          `json:"role"`
			Content json.RawMessage `json:"content"`
		} `json:"messages"`
	}
	json.Unmarshal(body, &q)
	thinking := q.Thinking == nil || q.Thinking.Type != "disabled"
	// afterTool: the message before is tool results alone, which a reply
	// with no thinking is refused after too (checked 2026-10-12, #1822)
	afterTool := false
	for i, m := range q.Messages {
		var blocks []struct {
			Type string `json:"type"`
		}
		json.Unmarshal(m.Content, &blocks)
		after := afterTool
		afterTool = m.Role == "user" && len(blocks) > 0
		thought, called := false, false
		for _, b := range blocks {
			afterTool = afterTool && b.Type == "tool_result"
			switch b.Type {
			case "redacted_thinking":
				return 422, `{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[` + string(rune('0'+i)) + "].content: unknown variant `redacted_thinking`, expected one of `text`, `image`, `document`, `thinking`, `server_tool_use`, `tool_use`, `tool_result`" + `","type":"invalid_request_error"}}`
			case "thinking":
				thought = true
			case "tool_use":
				called = true
			}
		}
		if thinking && m.Role == "assistant" && (called || after) && !thought {
			return 400, `{"error":{"message":"The ` + "`content[].thinking`" + ` in the thinking mode must be passed back to the API. (request_id: e689e65a-f4f1-4efb-a4e0-44b98b5325d4)","type":"invalid_request_error","param":null,"code":"invalid_request_error"}}`
		}
	}
	return 0, ""
}

const anthropicWholeOK = `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"Done."}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`

// a Claude Code conversation a routing group served with Claude first:
// Claude's redacted thinking before one call, Claude's signed thinking
// before another, a second step with none, and a reply with none
const claudeTurnsForDeepSeek = `{"model":"%s","max_tokens":32000,"stream":false,"thinking":{"type":"adaptive"},
	"tools":[{"name":"Bash","input_schema":{"type":"object","properties":{"command":{"type":"string"}}}}],
	"messages":[
	{"role":"user","content":"fix the login bug"},
	{"role":"assistant","content":[{"type":"redacted_thinking","data":"EmwKAhgBEgy3va3pzix"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]},
	{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"main.go"}]},
	{"role":"assistant","content":[{"type":"thinking","thinking":"Read main.go.","signature":"EqQBCkgIBxABGAIiQL"},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"cat main.go"}}]},
	{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"package main"}]},
	{"role":"assistant","content":[{"type":"text","text":"Checking the test."},{"type":"tool_use","id":"toolu_3","name":"Bash","input":{"command":"go test"}}]},
	{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_3","content":"ok"}]},
	{"role":"assistant","content":[{"type":"redacted_thinking","data":"EmwKAhgBEgy3va3pzix"},{"type":"thinking","thinking":"Fine.","signature":"EqQBCkgIBxAB"},{"type":"text","text":"It passes."}]},
	{"role":"user","content":"thanks, now commit"}]}`

func sentAnthropicMessages(t *testing.T, body []byte) []json.RawMessage {
	t.Helper()
	var q struct {
		Messages []json.RawMessage `json:"messages"`
	}
	if err := json.Unmarshal(body, &q); err != nil {
		t.Fatalf("%v: %s", err, body)
	}
	return q.Messages
}

// A Claude Code routing group that falls from Claude to DeepSeek's
// Anthropic endpoint (#1611): each tool_use turn goes with a thinking
// block, Claude's redacted thinking as a space (or left out beside thinking
// of its own), so DeepSeek takes it; signed thinking, the reply and the
// user's messages go as they were sent.
func TestDeepSeekAnthropicToolTurnsGetThinking(t *testing.T) {
	f := &fake{t: t, ctype: "application/json", reply: anthropicWholeOK, refuse: deepseekRefuseAnthropic}
	up := setup(t, provider.Anthropic, f)
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Key: "k", Models: []string{"deepseek-flash", "glm-5.2", "claude-sonnet-4-6"}, Anthropic: up.URL}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "cc", Name: "cc", Members: []string{"deepseek/deepseek-flash"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	srv := New()
	for _, model := range []string{"deepseek/deepseek-flash", "group/cc"} {
		f.calls = 0
		code, body := postTo(t, srv, "/v1/messages", strings.Replace(claudeTurnsForDeepSeek, "%s", model, 1))
		if code != 200 || f.calls != 1 {
			t.Fatalf("%s: %d after %d calls: %s", model, code, f.calls, body)
		}
		ms := sentAnthropicMessages(t, f.got)
		sameJSON(t, ms[0], `{"role":"user","content":"fix the login bug"}`)
		sameJSON(t, ms[1], `{"role":"assistant","content":[{"type":"thinking","thinking":" "},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}`)
		sameJSON(t, ms[3], `{"role":"assistant","content":[{"type":"thinking","thinking":"Read main.go.","signature":"EqQBCkgIBxABGAIiQL"},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"cat main.go"}}]}`)
		sameJSON(t, ms[5], `{"role":"assistant","content":[{"type":"thinking","thinking":" "},{"type":"text","text":"Checking the test."},{"type":"tool_use","id":"toolu_3","name":"Bash","input":{"command":"go test"}}]}`)
		sameJSON(t, ms[7], `{"role":"assistant","content":[{"type":"thinking","thinking":" "},{"type":"thinking","thinking":"Fine.","signature":"EqQBCkgIBxAB"},{"type":"text","text":"It passes."}]}`)
	}

	// another vendor's model, and a Claude model, on the same upstream are
	// sent the conversation as it came
	f.refuse = nil
	for _, model := range []string{"deepseek/glm-5.2", "deepseek/claude-sonnet-4-6"} {
		code, body := postTo(t, srv, "/v1/messages", strings.Replace(claudeTurnsForDeepSeek, "%s", model, 1))
		if code != 200 {
			t.Fatalf("%s: %d: %s", model, code, body)
		}
		if strings.Count(string(f.got), `"redacted_thinking"`) != 2 || strings.Contains(string(f.got), `"thinking":" "`) {
			t.Errorf("%s was sent changed thinking: %s", model, f.got)
		}
	}
}

// The rule holds with thinking left out (DeepSeek's default is to think):
// Claude Code's body without thinking is told thinking is off, and a
// tool_use turn still gets its block.
func TestDeepSeekAnthropicNoThinkingField(t *testing.T) {
	f := &fake{t: t, ctype: "application/json", reply: anthropicWholeOK, refuse: deepseekRefuseAnthropic}
	up := setup(t, provider.Anthropic, f)
	if err := provider.Save(provider.Provider{ID: "ds", Name: "DS", Key: "k", Models: []string{"deepseek-v4-pro"}, Anthropic: up.URL}); err != nil {
		t.Fatal(err)
	}
	body := strings.Replace(strings.Replace(claudeTurnsForDeepSeek, `"thinking":{"type":"adaptive"},`, "", 1), "%s", "ds/deepseek-v4-pro", 1)
	if code, b := postTo(t, New(), "/v1/messages", body); code != 200 {
		t.Fatalf("%d: %s", code, b)
	}
	if ms := sentAnthropicMessages(t, f.got); !strings.HasPrefix(string(ms[1]), `{"content":[{"type":"thinking","thinking":" "}`) {
		t.Errorf("first tool turn sent as %s", ms[1])
	}
}

// Translated onto DeepSeek's Anthropic endpoint (a Chat client, the
// provider giving only a Messages URL), a tool call with no thinking goes
// after a thinking block holding a space, and one with DeepSeek's own
// reasoning_content keeps its text (#1611); another model is sent neither.
func TestTranslatedDeepSeekAnthropicToolTurn(t *testing.T) {
	f := &fake{t: t, refuse: deepseekRefuseAnthropic, reply: sse(
		`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_1","model":"m1","usage":{"input_tokens":7}}}`,
		`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
		`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"12:01"}}`,
		`data: {"type":"content_block_stop","index":0}`,
		`data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}`,
		`data: {"type":"message_stop"}`)}
	up := setup(t, provider.Anthropic, f)
	if err := provider.Save(provider.Provider{ID: "ds", Name: "DS", Key: "k", Models: []string{"deepseek-flash", "glm-5.2"}, Anthropic: up.URL}); err != nil {
		t.Fatal(err)
	}
	srv := New()
	chat := `{"model":"%s","stream":false,"reasoning_effort":"high","tools":[{"type":"function","function":{"name":"get_time","parameters":{"type":"object","properties":{}}}}],"messages":[
		{"role":"user","content":"what time is it? twice"},
		{"role":"assistant","content":null,"tool_calls":[` + timeCall + `]},
		{"role":"tool","tool_call_id":"call_1","content":"12:00"},
		{"role":"assistant","content":null,"tool_calls":[{"id":"call_2","type":"function","function":{"name":"get_time","arguments":"{}"}}],"reasoning_content":"Once more."},
		{"role":"tool","tool_call_id":"call_2","content":"12:01"}]}`
	code, body := postTo(t, srv, "/v1/chat/completions", strings.Replace(chat, "%s", "ds/deepseek-flash", 1))
	if code != 200 || f.calls != 1 {
		t.Fatalf("%d after %d calls: %s", code, f.calls, body)
	}
	var thinking []string
	for _, raw := range sentAnthropicMessages(t, f.got) {
		var m struct {
			Role    string `json:"role"`
			Content []aBlock
		}
		json.Unmarshal(raw, &m)
		for _, b := range m.Content {
			if b.Type == "thinking" {
				thinking = append(thinking, b.Thinking)
			}
		}
	}
	if got := strings.Join(thinking, "|"); got != " |Once more." {
		t.Errorf("thinking sent %q, want %q: %s", got, " |Once more.", f.got)
	}

	f.refuse = nil
	if code, body := postTo(t, srv, "/v1/chat/completions", strings.Replace(chat, "%s", "ds/glm-5.2", 1)); code != 200 {
		t.Fatalf("glm: %d %s", code, body)
	}
	if strings.Contains(string(f.got), `"type":"thinking"`) {
		t.Errorf("glm was given thinking: %s", f.got)
	}
}

// withThinkingOnToolTurns leaves a body with nothing to change byte for
// byte.
func TestWithThinkingOnToolTurnsUnchanged(t *testing.T) {
	for _, b := range []string{
		`{"model":"deepseek-flash","messages":[{"role":"user","content":"hi"}]}`,
		`{"model":"deepseek-flash",  "messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"x"},{"type":"tool_use","id":"t","name":"n","input":{}}]}]}`,
		`{"messages":[{"role":"assistant","content":"plain"}]}`,
	} {
		if got := string(withThinkingOnToolTurns([]byte(b))); got != b {
			t.Errorf("%s\nbecame %s", b, got)
		}
	}
	if anthropicReplaysThinking("api.anthropic.com", "deepseek-flash") || anthropicReplaysThinking("api.deepseek.com", "claude-sonnet-4-6") || anthropicReplaysThinking("api.moonshot.ai", "kimi-k2") {
		t.Error("replays thinking outside DeepSeek")
	}
	if !anthropicReplaysThinking("api.deepseek.com", "deepseek-flash") || !anthropicReplaysThinking("relay.example", "DeepSeek-V4-Pro") {
		t.Error("DeepSeek not replayed")
	}
}
