package gateway

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
)

// zedUpstream stands in for cloud.zed.dev/completions: reply answers each
// request (by its envelope), and the account's model token is "llm-<n>",
// n counting how many were minted.
func zedUpstream(t *testing.T, vendor string, reply func(w http.ResponseWriter, body []byte, auth string)) *atomic.Int32 {
	t.Helper()
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	var minted atomic.Int32
	minted.Store(1)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/completions" || r.Method != http.MethodPost {
			t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
			w.WriteHeader(404)
			return
		}
		if r.Header.Get("x-zed-version") == "" || r.Header.Get("x-zed-client-supports-status-messages") != "true" ||
			!strings.HasPrefix(r.Header.Get("User-Agent"), "Zed/") {
			t.Errorf("not asked as Zed asks: %v", r.Header)
		}
		b, _ := io.ReadAll(r.Body)
		reply(w, b, r.Header.Get("Authorization"))
	}))
	t.Cleanup(srv.Close)
	oldTok, oldOf, oldBase := zedToken, zedProviderOf, zedBase
	zedToken = func(_ context.Context, user string, renew bool) (string, error) {
		if user != "octo" {
			t.Errorf("token for %q", user)
		}
		if renew {
			minted.Add(1)
		}
		return "llm-" + string(rune('0'+minted.Load())), nil
	}
	zedProviderOf = func(string, string) string { return vendor }
	zedBase = func() string { return srv.URL }
	t.Cleanup(func() { zedToken, zedProviderOf, zedBase = oldTok, oldOf, oldBase })
	return &minted
}

func zedLines(w http.ResponseWriter, lines ...string) {
	w.Header().Set("x-zed-server-supports-status-messages", "true")
	w.WriteHeader(200)
	io.WriteString(w, `{"status":"started"}`+"\n")
	for _, l := range lines {
		io.WriteString(w, l+"\n")
	}
}

func zedEvent(ev string) string { return `{"event":` + ev + `}` }

const zedEnded = `{"status":"stream_ended"}`

func zedServe(t *testing.T, from provider.Protocol, model, body string) (int, string, string) {
	t.Helper()
	s := New()
	w := httptest.NewRecorder()
	r := httptest.NewRequest("POST", "/", strings.NewReader(body))
	var u Usage
	status, msg := s.serveZed(w, r, from, provider.Provider{ID: "zed", Account: &provider.Account{Agent: "zed", User: "octo"}}, model, []byte(body), &u)
	return status, msg, w.Body.String()
}

// Each of Zed's providers is asked in its own API inside Zed's envelope, and
// its own events come back through the client's protocol.
func TestServeZedProviders(t *testing.T) {
	for _, tt := range []struct {
		vendor, model string
		check         func(t *testing.T, req gjson.Result)
		lines         []string
	}{
		{"anthropic", "claude-sonnet-4-5", func(t *testing.T, req gjson.Result) {
			if req.Get("stream").Exists() || req.Get("model").String() != "claude-sonnet-4-5" || req.Get("messages.0.content").Raw == "" || req.Get("max_tokens").Int() == 0 {
				t.Errorf("anthropic request %s", req.Raw)
			}
		}, []string{
			zedEvent(`{"type":"message_start","message":{"id":"m","role":"assistant","usage":{"input_tokens":7,"output_tokens":0}}}`),
			zedEvent(`{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`),
			zedEvent(`{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello from Claude"}}`),
			zedEvent(`{"type":"content_block_stop","index":0}`),
			zedEvent(`{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}`),
			zedEvent(`{"type":"message_stop"}`),
			zedEnded,
		}},
		{"open_ai", "gpt-5", func(t *testing.T, req gjson.Result) {
			if !req.Get("stream").Bool() || req.Get("input").Raw == "" || req.Get("model").String() != "gpt-5" {
				t.Errorf("responses request %s", req.Raw)
			}
		}, []string{
			zedEvent(`{"type":"response.created","response":{"id":"r"}}`),
			zedEvent(`{"type":"response.output_text.delta","item_id":"i","output_index":0,"content_index":0,"delta":"Hello from GPT"}`),
			zedEvent(`{"type":"response.completed","response":{"id":"r","status":"completed","usage":{"input_tokens":5,"output_tokens":3}}}`),
			zedEnded,
		}},
		{"x_ai", "grok-4", func(t *testing.T, req gjson.Result) {
			if req.Get("max_tokens").Exists() || req.Get("max_completion_tokens").Int() != 64 || req.Get("messages.0").Raw == "" {
				t.Errorf("xAI request %s", req.Raw)
			}
		}, []string{
			zedEvent(`{"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello from Grok"}}]}`),
			zedEvent(`{"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3}}`),
			zedEnded,
		}},
		{"google", "gemini-2.5-pro", func(t *testing.T, req gjson.Result) {
			if req.Get("model").String() != "models/gemini-2.5-pro" || req.Get("contents.0.parts.0.text").String() != "hi" || req.Get("request").Exists() {
				t.Errorf("Gemini request %s", req.Raw)
			}
		}, []string{
			zedEvent(`{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello from Gemini"}]}}]}`),
			zedEvent(`{"candidates":[{"content":{"role":"model","parts":[{"text":""}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3}}`),
			zedEnded,
		}},
	} {
		t.Run(tt.vendor, func(t *testing.T) {
			zedUpstream(t, tt.vendor, func(w http.ResponseWriter, b []byte, auth string) {
				env := gjson.ParseBytes(b)
				if env.Get("provider").String() != tt.vendor || env.Get("model").String() != tt.model || !env.Get("provider_request").IsObject() {
					t.Errorf("envelope %s", b)
				}
				if auth != "Bearer llm-1" {
					t.Errorf("signed %q", auth)
				}
				tt.check(t, env.Get("provider_request"))
				zedLines(w, tt.lines...)
			})
			for _, stream := range []bool{true, false} {
				body := `{"model":"zed/` + tt.model + `","max_tokens":64,"stream":` + map[bool]string{true: "true", false: "false"}[stream] + `,"messages":[{"role":"user","content":"hi"}]}`
				status, msg, out := zedServe(t, provider.Chat, tt.model, body)
				if status != 200 || !strings.Contains(out, "Hello from") {
					t.Fatalf("stream=%v: %d %q\n%s", stream, status, msg, out)
				}
			}
		})
	}
}

// A model token Zed calls stale is minted again and the request asked once
// more; a failure the stream reports, a refused plan and a reply cut short
// all reach the client as errors.
func TestServeZedFailures(t *testing.T) {
	body := `{"model":"m","stream":false,"messages":[{"role":"user","content":"hi"}]}`
	text := zedEvent(`{"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"ok"}}]}`)

	var asked atomic.Int32
	minted := zedUpstream(t, "x_ai", func(w http.ResponseWriter, _ []byte, auth string) {
		if asked.Add(1) == 1 {
			w.Header().Set("x-zed-expired-token", "true")
			w.WriteHeader(401)
			return
		}
		if auth != "Bearer llm-2" {
			t.Errorf("retried with %q", auth)
		}
		zedLines(w, text, zedEnded)
	})
	if status, msg, out := zedServe(t, provider.Chat, "grok-4", body); status != 200 || !strings.Contains(out, "ok") || minted.Load() != 2 {
		t.Fatalf("stale token: %d %q %s (minted %d)", status, msg, out, minted.Load())
	}

	for _, tt := range []struct {
		name   string
		reply  func(w http.ResponseWriter)
		status int
		want   string
	}{
		{"payment", func(w http.ResponseWriter) { w.WriteHeader(402) }, 402, "payment required"},
		{"upstream", func(w http.ResponseWriter) {
			w.WriteHeader(500)
			io.WriteString(w, `{"code":"upstream_http_529","message":"Overloaded","upstream_status":529,"retry_after":2}`)
		}, 529, "Overloaded"},
		{"failed status", func(w http.ResponseWriter) {
			zedLines(w, `{"status":{"failed":{"code":"upstream_http_429","message":"slow down"}}}`)
		}, 429, "slow down"},
		{"cut short", func(w http.ResponseWriter) { zedLines(w, text) }, 502, "ended before"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			zedUpstream(t, "x_ai", func(w http.ResponseWriter, _ []byte, _ string) { tt.reply(w) })
			status, msg, out := zedServe(t, provider.Chat, "grok-4", body)
			if status != tt.status || !strings.Contains(msg+out, tt.want) {
				t.Fatalf("%d %q %s", status, msg, out)
			}
		})
	}
}

func TestZedRequestShapes(t *testing.T) {
	req := &Request{Model: "m", MaxTokens: 10, Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: "hi"}}}}}
	for vendor, proto := range map[string]provider.Protocol{"anthropic": provider.Anthropic, "open_ai": provider.Responses, "x_ai": provider.Chat, "google": provider.CodeAssist} {
		raw, p, err := zedRequest(req, vendor, "m")
		if err != nil || p != proto || !json.Valid(raw) {
			t.Errorf("%s: %v %s %s", vendor, err, p, raw)
		}
	}
	if _, _, err := zedRequest(req, "mystery", "m"); err == nil {
		t.Error("an unknown provider was written")
	}
}

// Zed's cloud wants is_error on each tool result, false too, or every turn
// after a tool call is refused (yetone/magpie-releases#9).
func TestZedToolResultSaysIsError(t *testing.T) {
	req := &Request{Model: "m", MaxTokens: 10, Messages: []Message{
		{Role: "user", Parts: []Part{{Kind: Text, Text: "run echo hi"}}},
		{Role: "assistant", Parts: []Part{
			{Kind: ToolCall, ID: "toolu_1", Name: "bash", Args: json.RawMessage(`{"command":"echo hi","n":12345678901234567890}`)},
			{Kind: ToolCall, ID: "toolu_2", Name: "bash", Args: json.RawMessage(`{"command":"false"}`)},
		}},
		// toolu_9 answers no call the request has: it goes as the text it
		// said (pairToolBlocks), as the cloud refuses it as Anthropic does
		{Role: "user", Parts: []Part{{Kind: ToolResult, CallID: "toolu_1", Text: "hi"}, {Kind: ToolResult, CallID: "toolu_2", Text: "no", IsError: true}, {Kind: ToolResult, CallID: "toolu_9", Text: "late"}}},
	}}
	raw, _, err := zedRequest(req, "anthropic", "claude-sonnet-5-5")
	if err != nil {
		t.Fatal(err)
	}
	var got struct {
		Messages []struct {
			Content []map[string]any `json:"content"`
		} `json:"messages"`
	}
	if err := json.Unmarshal(raw, &got); err != nil {
		t.Fatal(err)
	}
	var errs []any
	var texts []string
	for _, m := range got.Messages {
		for _, b := range m.Content {
			if b["type"] == "tool_result" {
				v, ok := b["is_error"]
				if !ok {
					t.Errorf("tool result without is_error: %v", b)
				}
				errs = append(errs, v)
			}
			if b["type"] == "text" {
				if s, ok := b["text"].(string); ok {
					texts = append(texts, s)
				}
			}
		}
	}
	if !bytes.Contains(raw, []byte(`12345678901234567890`)) {
		t.Errorf("a tool input number changed: %s", raw)
	}
	if len(errs) != 2 || errs[0] != false || errs[1] != true {
		t.Errorf("is_error: %v in %s", errs, raw)
	}
	if !slices.Contains(texts, "late") {
		t.Errorf("what the lost result said is not kept: %v in %s", texts, raw)
	}
}
