package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A reply whose body names the member (Settings' MemberModel, or a
// request's X-Magpie-Response-Model: member) went through two writers
// that each hold a whole body until the handler is done: the one that
// names the member, outside, and the one that puts masked values back,
// inside it. The namer wrote what it held first, nothing yet, and the
// values came back after it, into a buffer no one wrote: a non-streamed
// reply with a masked value in its request was a 200 with Content-Length
// 0, while a stream, written as it goes, was whole (#4129, versun).
// Here, every API a reply comes back whole in, through a real server so
// the length the agent is told is checked against what it gets.
func TestMaskedWholeReplyNamesMember(t *testing.T) {
	const phone = "13800138000"
	const ask = "复述这句话：我的手机号是 " + phone + "。"
	placeholder := regexp.MustCompile(`\{\{[A-Z_]+_[a-z2-7]{8}\}\}`)
	var leaked []string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		if strings.Contains(string(b), phone) {
			leaked = append(leaked, string(b))
		}
		// the vendor says back what it was given: the placeholder
		text, _ := json.Marshal("我的手机号是 " + placeholder.FindString(string(b)) + "。")
		stream := strings.Contains(string(b), `"stream":true`) || strings.Contains(r.URL.Path, "stream")
		var out string
		switch {
		case strings.HasSuffix(r.URL.Path, "/messages") && stream:
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, sse(
				"event: message_start\ndata: "+`{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[],"usage":{"input_tokens":3,"output_tokens":0}}}`,
				"event: content_block_start\ndata: "+`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
				"event: content_block_delta\ndata: "+`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":`+string(text)+`}}`,
				"event: content_block_stop\ndata: "+`{"type":"content_block_stop","index":0}`,
				"event: message_delta\ndata: "+`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}`,
				"event: message_stop\ndata: "+`{"type":"message_stop"}`))
			return
		case strings.HasSuffix(r.URL.Path, "/messages"):
			out = `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":` + string(text) + `}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":5}}`
		case strings.Contains(r.URL.Path, "enerateContent"):
			out = `{"candidates":[{"content":{"role":"model","parts":[{"text":` + string(text) + `}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":5},"modelVersion":"m1"}`
			if stream {
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, sse("data: "+out))
				return
			}
		case stream:
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, sse(
				`data: {"id":"c1","object":"chat.completion.chunk","model":"m1","choices":[{"index":0,"delta":{"role":"assistant","content":`+string(text)+`}}]}`,
				`data: {"id":"c1","object":"chat.completion.chunk","model":"m1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":5}}`,
				"data: [DONE]"))
			return
		default:
			out = `{"id":"c1","object":"chat.completion","model":"m1","choices":[{"index":0,"message":{"role":"assistant","content":` + string(text) + `},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":5}}`
		}
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("Content-Length", strconv.Itoa(len(out)))
		io.WriteString(w, out)
	}))
	defer up.Close()

	chatAsk := `{"model":"MODEL","messages":[{"role":"user","content":"` + ask + `"}]}`
	for _, tc := range []struct {
		name, provider, path, body string
		header                     bool // asked by the request's header, not the setting
	}{
		{"chat, a group's member (the report)", "chat", "/v1/chat/completions", strings.Replace(chatAsk, "MODEL", "group/subagent", 1), false},
		{"chat, direct", "chat", "/v1/chat/completions", strings.Replace(chatAsk, "MODEL", "fake/m1", 1), false},
		{"chat, json_object", "chat", "/v1/chat/completions", `{"model":"fake/m1","response_format":{"type":"json_object"},"messages":[{"role":"user","content":"` + ask + `"}]}`, false},
		{"chat, asked by header", "chat", "/v1/chat/completions", strings.Replace(chatAsk, "MODEL", "fake/m1", 1), true},
		{"chat to an Anthropic vendor", "anthropic", "/v1/chat/completions", strings.Replace(chatAsk, "MODEL", "fake/m1", 1), false},
		{"chat to a Gemini vendor", "gemini", "/v1/chat/completions", strings.Replace(chatAsk, "MODEL", "group/subagent", 1), false},
		{"messages", "anthropic", "/v1/messages", `{"model":"fake/m1","max_tokens":100,"messages":[{"role":"user","content":"` + ask + `"}]}`, false},
		{"messages to a chat vendor", "chat", "/v1/messages", `{"model":"fake/m1","max_tokens":100,"messages":[{"role":"user","content":"` + ask + `"}]}`, false},
		{"responses", "chat", "/v1/responses", `{"model":"fake/m1","input":"` + ask + `"}`, false},
		{"gemini", "gemini", "/v1beta/models/fake/m1:generateContent", `{"contents":[{"role":"user","parts":[{"text":"` + ask + `"}]}]}`, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			fresh(t)
			p := provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}}
			switch tc.provider {
			case "chat":
				p.Chat = up.URL + "/v1"
			case "anthropic":
				p.Anthropic = up.URL
			case "gemini":
				p.Gemini = up.URL
			}
			if err := provider.Save(p); err != nil {
				t.Fatal(err)
			}
			if err := provider.SaveGroup(provider.Group{ID: "subagent", Name: "subagent", Members: []string{"fake/m1"}, Routing: provider.Ordered}); err != nil {
				t.Fatal(err)
			}
			// the reporter's settings
			if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactShape: true, MemberModel: !tc.header,
				RedactActions: map[string]string{"PERSON": "warn", "ID_CARD": "warn", "BIRTHDAY": "warn", "USERNAME": "warn"}}); err != nil {
				t.Fatal(err)
			}
			srv := httptest.NewServer(New().Handler())
			defer srv.Close()
			req, _ := http.NewRequest("POST", srv.URL+tc.path, strings.NewReader(tc.body))
			req.Header.Set("Content-Type", "application/json")
			if tc.header {
				req.Header.Set(responseModelHeader, "member")
			}
			res, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			defer res.Body.Close()
			b, err := io.ReadAll(res.Body)
			if err != nil {
				t.Fatalf("read: %v (%d bytes)", err, len(b))
			}
			if len(leaked) > 0 {
				t.Fatalf("the vendor was sent the number: %s", leaked)
			}
			if res.StatusCode != 200 || !strings.Contains(string(b), phone) {
				t.Fatalf("%d, Content-Length %q, the number not put back: %q", res.StatusCode, res.Header.Get("Content-Length"), b)
			}
			if placeholder.Match(b) {
				t.Fatalf("a placeholder came back: %s", b)
			}
			if cl := res.Header.Get("Content-Length"); cl != "" && cl != strconv.Itoa(len(b)) {
				t.Fatalf("Content-Length %s, body %d bytes", cl, len(b))
			}
			if !strings.Contains(string(b), "fake/m1") {
				t.Fatalf("the member isn't named: %s", b)
			}
		})
	}
}

// A writer inside the namer that holds a body longer than it, and writes
// it after the namer's finish, still has it reach the agent, named: the
// namer has no finish left to write it from its buffer (#4129).
func TestMemberWriterWrittenAfterFinish(t *testing.T) {
	for _, tc := range []struct {
		name, ct, body, want string
	}{
		{"whole", "application/json", `{"model":"m1","x":1}`, `{"model":"fake/m1","x":1}`},
		{"a stream's last line, cut", "text/event-stream", "data: {\"model\":\"m1\"}\n\ndata: [DO", "data: {\"model\":\"fake/m1\"}\n\ndata: [DO"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			rec := httptest.NewRecorder()
			m := &memberWriter{w: rec}
			m.setMember("fake/m1")
			m.Header().Set("Content-Type", tc.ct)
			m.WriteHeader(200)
			m.finish()
			io.WriteString(m, tc.body)
			if got := rec.Body.String(); got != tc.want {
				t.Fatalf("got %q, want %q", got, tc.want)
			}
		})
	}
}
