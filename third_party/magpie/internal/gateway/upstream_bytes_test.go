package gateway

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The bodies of these requests, as a vendor gets them, were saved from
// the gateway before it read request bodies with decodeJSON instead of
// json.Unmarshal (#1532, #1654): reading a body faster changes no byte
// of what is sent. UPDATE_UPSTREAM_BODIES=1 saves them again.
func TestUpstreamBodiesUnchanged(t *testing.T) {
	const (
		aReply = `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":1}}`
		cReply = `{"id":"c1","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":1}}`
		rReply = `{"id":"r1","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":7,"output_tokens":1}}`
		png    = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
	)
	anthropicBody := `{ "max_tokens" : 1024,"model":"fake/m1", "metadata":{"user_id":"u_1"},
 "system":[{"type":"text","text":"You are é \/ \"careful\"\n\ttabs","cache_control":{"type":"ephemeral"}}],
 "tools":[{"name":"Read","description":"Reads a file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}],
 "messages":[{"role":"user","content":"read it"},
  {"role":"assistant","content":[{"type":"thinking","thinking":"hmm","signature":"c2ln"},{"type":"text","text":"ok 漢字 😀"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"path":"/a b/c.go","n":1.50e2}}]},
  {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"line\u0001one\r\nline two"}]},{"type":"text","text":"go on"}]}],
 "temperature":0.70, "stream":false, "unknown_field":{"kept":[1,2,{"x":null}]}}`
	anthropicImage := `{"model":"fake/m1","max_tokens":64,"messages":[{"role":"user","content":[{"type":"text","text":"what is this"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"` + png + `"}}]}]}`
	anthropicThinking := `{"model":"fake/m1","max_tokens":2048,"thinking":{"type":"enabled","budget_tokens":1024},"messages":[{"role":"user","content":"think"}],"model":"fake/m1"}`
	chatBody := `{"model":"fake/m1","messages":[{"role":"system","content":"be brief"},{"role":"user","content":[{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,` + png + `"}}]},
 {"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"Read","arguments":"{\"path\":\"/x\"}"}}]},
 {"role":"tool","tool_call_id":"call_1","content":"file \"body\"\n"},{"role":"user","content":"go on é"}],
 "tools":[{"type":"function","function":{"name":"Read","description":"Reads","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}],
 "tool_choice":"auto","reasoning_effort":"high","temperature":1,"stop":["\n\n"],"stream":false,"user":"u"}`
	responsesBody := `{"model":"fake/m1","instructions":"be brief — ok","service_tier":"priority","reasoning":{"effort":"medium","summary":"auto"},
 "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"},{"type":"input_image","image_url":"data:image/png;base64,` + png + `"}]},
 {"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ZW5j"},
 {"type":"function_call","call_id":"call_1","name":"Read","arguments":"{\"path\":\"/x\"}"},
 {"type":"function_call_output","call_id":"call_1","output":"done\t\"x\""},
 {"type":"message","role":"user","content":"go on"}],
 "tools":[{"type":"function","name":"Read","description":"Reads","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}],
 "store":false,"stream":false,"include":["reasoning.encrypted_content"]}`
	responsesString := `{"model":"fake/m1","input":"just a string 😀","stream":false}`

	for _, tc := range []struct {
		name, path, model string
		proto             provider.Protocol // the vendor's protocol
		body              string
	}{
		{"anthropic", "/v1/messages", "fake/m1", provider.Anthropic, anthropicBody},
		{"anthropic-group", "/v1/messages", "group/g", provider.Anthropic, anthropicBody},
		{"anthropic-image", "/v1/messages", "fake/m1", provider.Anthropic, anthropicImage},
		{"anthropic-thinking", "/v1/messages", "fake/m1", provider.Anthropic, anthropicThinking},
		{"anthropic-to-chat", "/v1/messages", "fake/m1", provider.Chat, anthropicBody},
		{"anthropic-to-responses", "/v1/messages", "group/g", provider.Responses, anthropicImage},
		{"chat", "/v1/chat/completions", "fake/m1", provider.Chat, chatBody},
		{"chat-group", "/v1/chat/completions", "group/g", provider.Chat, chatBody},
		{"chat-to-anthropic", "/v1/chat/completions", "fake/m1", provider.Anthropic, chatBody},
		{"responses", "/v1/responses", "fake/m1", provider.Responses, responsesBody},
		{"responses-string", "/v1/responses", "group/g", provider.Responses, responsesString},
		{"responses-to-chat", "/v1/responses", "fake/m1", provider.Chat, responsesBody},
		{"responses-to-anthropic", "/v1/responses", "fake/m1", provider.Anthropic, responsesBody},
		{"large-anthropic", "/v1/messages", "fake/m1", provider.Anthropic, string(largeConversation(provider.Anthropic, "fake/m1", 30_000))},
		{"large-anthropic-group", "/v1/messages", "group/g", provider.Anthropic, string(largeConversation(provider.Anthropic, "group/g", 30_000))},
		{"large-chat", "/v1/chat/completions", "fake/m1", provider.Chat, string(largeConversation(provider.Chat, "fake/m1", 30_000))},
		{"large-responses", "/v1/responses", "fake/m1", provider.Responses, string(largeConversation(provider.Responses, "fake/m1", 30_000))},
		{"large-responses-to-chat", "/v1/responses", "fake/m1", provider.Chat, string(largeConversation(provider.Responses, "fake/m1", 30_000))},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("XDG_CONFIG_HOME", t.TempDir())
			t.Setenv("XDG_CACHE_HOME", t.TempDir())
			f := &fake{ctype: "application/json"}
			up := httptest.NewServer(f)
			defer up.Close()
			p := provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}}
			// a request in another protocol than the vendor's is streamed to it
			streamed := !strings.HasSuffix(tc.path, map[provider.Protocol]string{provider.Chat: "/chat/completions", provider.Responses: "/responses", provider.Anthropic: "/messages"}[tc.proto])
			switch tc.proto {
			case provider.Chat:
				p.Chat, f.reply = up.URL+"/v1", cReply
				if streamed {
					f.reply = sse(`data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":1}}`, `data: [DONE]`)
				}
			case provider.Responses:
				p.Responses, f.reply = up.URL+"/v1", rReply
				if streamed {
					f.reply = sse(`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
						`data: {"type":"response.output_text.delta","delta":"ok"}`,
						`data: {"type":"response.completed","response":{"id":"r1","status":"completed","usage":{"input_tokens":7,"output_tokens":1}}}`)
				}
			case provider.Anthropic:
				p.Anthropic, f.reply = up.URL, aReply
				if streamed {
					f.reply = sse(`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[],"usage":{"input_tokens":7,"output_tokens":0}}}`,
						`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
						`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}`,
						`event: content_block_stop`+"\n"+`data: {"type":"content_block_stop","index":0}`,
						`event: message_delta`+"\n"+`data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}`,
						`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
				}
			}
			if streamed {
				f.ctype = ""
			}
			if err := provider.Save(p); err != nil {
				t.Fatal(err)
			}
			if err := provider.SaveGroup(provider.Group{ID: "g", Name: "G", Members: []string{"fake/m1"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			rec := httptest.NewRecorder()
			New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", tc.path, bytes.NewReader([]byte(tc.body))))
			if rec.Code != 200 || f.calls != 1 {
				t.Fatalf("%d after %d calls: %s", rec.Code, f.calls, rec.Body.String())
			}
			// a big body is kept by its hash
			got := f.got
			name := filepath.Join("testdata", "upstream_bodies", tc.name+".json")
			if len(got) > 16<<10 {
				sum := sha256.Sum256(got)
				got = []byte(hex.EncodeToString(sum[:]) + "\n")
				name = filepath.Join("testdata", "upstream_bodies", tc.name+".sha256")
			}
			if os.Getenv("UPDATE_UPSTREAM_BODIES") == "1" {
				if err := os.MkdirAll(filepath.Dir(name), 0o755); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(name, got, 0o644); err != nil {
					t.Fatal(err)
				}
				return
			}
			want, err := os.ReadFile(name)
			if err != nil {
				t.Fatal(err)
			}
			// a Windows checkout may have made its line ends CRLF; none of
			// these bodies holds a CR of its own
			want = bytes.ReplaceAll(want, []byte("\r\n"), []byte("\n"))
			if !bytes.Equal(got, want) {
				t.Errorf("the vendor got other bytes than before:\n got %s\nwant %s", got, want)
			}
		})
	}
}
