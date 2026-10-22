package gateway

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// toolsOffNoTools is the same ask, tools off with none declared, in each
// client protocol magpie translates to Responses.
var toolsOffNoTools = []struct{ name, path, body string }{
	{"chat", "/v1/chat/completions", `{"model":"fake/m1","stream":false,"tool_choice":"none","messages":[{"role":"user","content":"hi"}]}`},
	{"anthropic", "/v1/messages", `{"model":"fake/m1","max_tokens":64,"tool_choice":{"type":"none"},"messages":[{"role":"user","content":"hi"}]}`},
	{"gemini", "/v1beta/models/fake/m1:generateContent", `{"toolConfig":{"functionCallingConfig":{"mode":"NONE"}},"contents":[{"role":"user","parts":[{"text":"hi"}]}]}`},
}

var noneReply = sse(
	`data: {"type":"response.created","response":{"id":"resp_n","status":"in_progress","output":[]}}`,
	`data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"m1","delta":"OK"}`,
	`data: {"type":"response.completed","response":{"id":"resp_n","status":"completed","output":[]}}`)

// #3110: a Chat, Anthropic or Gemini request that turned tools off, with
// no tools, reaches a provider plugin on Responses still saying "none",
// so a plugin adding a server-side tool (Grok's x_search) can tell it
// from no choice. The plugin drops it before Grok (0.1.18).
func TestToolsOffWithoutToolsReachesPlugin(t *testing.T) {
	var mu sync.Mutex
	var sent []map[string]any
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, noneReply)
	}))
	for _, c := range toolsOffNoTools {
		t.Run(c.name, func(t *testing.T) {
			mu.Lock()
			sent = nil
			mu.Unlock()
			path := strings.Replace(c.path, "fake/m1", pid+"/fake-resp", 1)
			body := strings.Replace(c.body, "fake/m1", pid+"/fake-resp", 1)
			rec := httptest.NewRecorder()
			New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
			if rec.Code != 200 {
				t.Fatalf("%d %s", rec.Code, rec.Body)
			}
			mu.Lock()
			defer mu.Unlock()
			if len(sent) != 1 {
				t.Fatalf("asked %d times", len(sent))
			}
			if sent[0]["tool_choice"] != "none" {
				t.Errorf("tool_choice = %v, want \"none\"; the plugin got %v", sent[0]["tool_choice"], sent[0])
			}
			if tools, _ := sent[0]["tools"].([]any); len(tools) > 0 {
				t.Errorf("tools added: %v", sent[0])
			}
		})
	}
}

// Any other provider plugin on Responses is sent no lone tool_choice: its
// vendor hasn't been tried with one, and a 400 there would be a regression.
func TestToolsOffWithoutToolsNotSentToOtherPlugins(t *testing.T) {
	var mu sync.Mutex
	var sent [][]byte
	pid := besideFake(t, "factory", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		sent = append(sent, b)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, noneReply)
	}))
	for _, c := range toolsOffNoTools {
		t.Run(c.name, func(t *testing.T) {
			mu.Lock()
			sent = nil
			mu.Unlock()
			path := strings.Replace(c.path, "fake/m1", pid+"/fake-resp", 1)
			body := strings.Replace(c.body, "fake/m1", pid+"/fake-resp", 1)
			rec := httptest.NewRecorder()
			New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
			mu.Lock()
			defer mu.Unlock()
			if rec.Code != 200 || len(sent) != 1 || strings.Contains(string(sent[0]), `"tool_choice"`) {
				t.Errorf("%d %s\nplugin got %q", rec.Code, rec.Body, sent)
			}
		})
	}
}

// A vendor's Responses API is sent no tool_choice beside no tools, as
// before #3110: it has no use for one, and some turn it away.
func TestToolsOffWithoutToolsNotSentToVendor(t *testing.T) {
	for _, c := range toolsOffNoTools {
		t.Run(c.name, func(t *testing.T) {
			f := &fake{t: t, reply: noneReply}
			setup(t, provider.Responses, f)
			code, out := post(t, c.path, c.body)
			if code != 200 || strings.Contains(string(f.got), `"tool_choice"`) {
				t.Errorf("%d %s\nupstream got %s", code, out, f.got)
			}
		})
	}
}

// xAI turns away a tool_choice with no tools beside it (#378), so the
// "none" #3110 keeps is left out there even past the vendor rule above.
func TestToolsOffWithoutToolsLeftOutForXAI(t *testing.T) {
	for _, c := range toolsOffNoTools {
		t.Run(c.name, func(t *testing.T) {
			f := &fake{t: t, reply: noneReply}
			f.refuse = func(body []byte) (int, string) {
				var q map[string]any
				json.Unmarshal(body, &q)
				if tools, _ := q["tools"].([]any); len(tools) == 0 && q["tool_choice"] != nil {
					return 400, `{"code":"Client specified an invalid argument","error":"Invalid request content: A tool_choice was set on the request but no tools were specified."}`
				}
				return 0, ""
			}
			setup(t, provider.Responses, f)
			if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1"}, Responses: "http://api.x.ai/v1"}); err != nil {
				t.Fatal(err)
			}
			up := httptest.NewServer(f)
			t.Cleanup(up.Close)
			s := New()
			addr := strings.TrimPrefix(up.URL, "http://")
			s.client = &http.Client{Transport: &http.Transport{DialContext: func(ctx context.Context, network, _ string) (net.Conn, error) {
				return (&net.Dialer{}).DialContext(ctx, network, addr)
			}}}
			rec := httptest.NewRecorder()
			s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", c.path, strings.NewReader(c.body)))
			if rec.Code != 200 || strings.Contains(string(f.got), `"tool_choice"`) {
				t.Errorf("%d %s\nxAI got %s", rec.Code, rec.Body.String(), f.got)
			}
		})
	}
}

// Each builder, given tools off and none declared: Responses says "none";
// Chat (OpenAI: "'tool_choice' is only allowed when 'tools' are
// specified"), Anthropic, Gemini and Code Assist send no tool choice at
// all, as before #3110.
func TestToolsOffWithoutToolsPerBuilder(t *testing.T) {
	r := &Request{Model: "m", ToolChoice: "none", Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: "hi"}}}}}
	gem, err := buildGemini(r, "upstream")
	if err != nil {
		t.Fatal(err)
	}
	for _, c := range []struct {
		name string
		body []byte
		want any
	}{
		{"responses", buildResponses(r, "upstream", "api.openai.com", false), "none"},
		{"chat", buildChat(r, "upstream", "api.openai.com", false), nil},
		{"anthropic", buildAnthropic(r, "upstream"), nil},
		{"gemini", gem, nil},
		{"codeassist", buildCodeAssist(r, "upstream", "gemini"), nil},
	} {
		t.Run(c.name, func(t *testing.T) {
			var q map[string]any
			if err := json.Unmarshal(c.body, &q); err != nil {
				t.Fatalf("%v: %s", err, c.body)
			}
			if q["tool_choice"] != c.want || q["tools"] != nil || strings.Contains(string(c.body), "toolConfig") {
				t.Errorf("tool_choice = %v, want %v: %s", q["tool_choice"], c.want, c.body)
			}
		})
	}
	// a choice other than none, with no tools, still goes unsaid
	for _, choice := range []string{"", "auto"} {
		r.ToolChoice = choice
		if b := buildResponses(r, "upstream", "api.openai.com", false); strings.Contains(string(b), `"tool_choice"`) {
			t.Errorf("%q: %s", choice, b)
		}
	}
}
