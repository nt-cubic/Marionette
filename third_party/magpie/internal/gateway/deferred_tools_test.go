package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// deferredVendor answers as Kimi Code's and DeepSeek's Anthropic endpoints
// answered the real requests (10-14): Kimi refuses any message holding a
// tool_addition, a tool_removal or a tool_reference with a 400 that names
// nothing; DeepSeek takes a tool_addition naming a tool, and refuses one
// defining it, a tool_removal and a message's tool_reference with a 422.
// Both take a tool_reference inside a tool_result.
type deferredVendor struct {
	deepseek bool
	mu       sync.Mutex
	bodies   []string
}

func (v *deferredVendor) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	v.mu.Lock()
	v.bodies = append(v.bodies, string(b))
	v.mu.Unlock()
	var m struct {
		Messages []struct {
			Content json.RawMessage `json:"content"`
		} `json:"messages"`
	}
	json.Unmarshal(b, &m)
	refused := ""
	for _, msg := range m.Messages {
		var blocks []struct {
			Type string `json:"type"`
			Tool struct {
				Type string `json:"type"`
			} `json:"tool"`
		}
		json.Unmarshal(msg.Content, &blocks)
		for _, bl := range blocks {
			switch {
			case bl.Type == "tool_addition" && v.deepseek && bl.Tool.Type == "tool_definition":
				refused = "tool_definition"
			case bl.Type == "tool_addition" && v.deepseek:
			case bl.Type == "tool_addition", bl.Type == "tool_removal", bl.Type == "tool_reference":
				refused = bl.Type
			}
		}
	}
	w.Header().Set("Content-Type", "application/json")
	switch {
	case refused != "" && v.deepseek:
		w.WriteHeader(422)
		io.WriteString(w, `{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[1].content: unknown variant `+"`"+refused+"`"+`, expected one of `+"`text`, `image`, `document`, `thinking`, `server_tool_use`, `tool_use`, `tool_result`, `web_search_tool_result`, `tool_addition`"+` at line 1 column 233 (request_id: 160c0348-740b-41e4-8a7f-6f4dc596c741)","type":"invalid_request_error","param":null,"code":"invalid_request_error"}}`)
	case refused != "":
		w.WriteHeader(400)
		io.WriteString(w, `{"error":{"type":"invalid_request_error","message":"Invalid request Error"},"type":"error"}`)
	default:
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`)
	}
}

func (v *deferredVendor) sent() []string {
	v.mu.Lock()
	defer v.mu.Unlock()
	return append([]string(nil), v.bodies...)
}

// Claude Code's deferred-tool blocks, as 2.1.296 builds them and as the
// reporter captured them.
const (
	ccAddRef = `{"type":"tool_addition","tool":{"type":"tool_reference","name":"Read"}}`
	ccAddDef = `{"type":"tool_addition","tool":{"type":"tool_definition","definition":{"name":"Read","description":"read a file","input_schema":{"type":"object"}}}}`
	ccRemove = `{"type":"tool_removal","tool":{"type":"tool_reference","name":"Read"}}`
	// the ToolSearch answer: stays wherever it goes
	ccSearchResult = `{"role":"assistant","content":[{"type":"tool_use","id":"toolu_01","name":"ToolSearch","input":{"query":"select:Read"}}]},` +
		`{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":[{"type":"tool_reference","tool_name":"Read"}]}]}`
)

func deferredBody(model, block string) string {
	return `{"model":"` + model + `","max_tokens":32,"stream":false,"tools":[{"name":"Read","description":"read a file","input_schema":{"type":"object"}}],"messages":[` +
		`{"role":"user","content":"hi"},` + ccSearchResult + `,` +
		`{"role":"system","content":[{"type":"text","text":"t"},` + block + `]},` +
		`{"role":"system","content":[` + block + `]},` +
		`{"role":"user","content":"ok?"}]}`
}

// #2042 (davisjiahao): once Claude Code's ToolSearch put a tool_addition
// in the conversation, every request to Kimi Code's Anthropic endpoint was
// 400 "Invalid request Error". Kimi is sent no deferred-tool block, a
// message left empty goes, and the rest (the system text, the ToolSearch
// answer's tool_reference) stays.
func TestKimiIsSentNoDeferredToolBlocks(t *testing.T) {
	fresh(t)
	up := &deferredVendor{}
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "kimi-code", Name: "Kimi Code", Preset: "kimi-code", Key: "k", Anthropic: srv.URL,
		Models: []string{"k3-256k"}}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	reporter := `{"model":"kimi-code/k3-256k","max_tokens":32,"messages":[{"role":"user","content":"hi"},` +
		`{"role":"system","content":[{"type":"text","text":"t"},{"tool":"Read","type":"tool_addition"}]},` +
		`{"role":"user","content":"ok?"}],"thinking":{"type":"disabled"}}`
	cases := map[string]string{
		"reporter's bytes":         reporter,
		"addition by name":         deferredBody("kimi-code/k3-256k", ccAddRef),
		"addition defined":         deferredBody("kimi-code/k3-256k", ccAddDef),
		"removal":                  deferredBody("kimi-code/k3-256k", ccRemove),
		"message's tool_reference": deferredBody("kimi-code/k3-256k", `{"type":"tool_reference","tool_name":"Read"}`),
	}
	for name, body := range cases {
		t.Run(name, func(t *testing.T) {
			n := len(up.sent())
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
			if rec.Code != 200 {
				t.Fatalf("status %d: %s", rec.Code, rec.Body)
			}
			sent := up.sent()[n:]
			if len(sent) != 1 {
				t.Fatalf("Kimi was asked %d times, want once", len(sent))
			}
			got := sent[0]
			for _, b := range []string{`"tool_addition"`, `"tool_removal"`} {
				if strings.Contains(got, b) {
					t.Errorf("Kimi was sent %s: %s", b, got)
				}
			}
			if !strings.Contains(got, `"text":"t"`) || !strings.Contains(got, `"text":"ok?"`) && !strings.Contains(got, `"content":"ok?"`) {
				t.Errorf("the conversation's text was lost: %s", got)
			}
			if name != "reporter's bytes" && !strings.Contains(got, `"tool_name":"Read"}]}`) {
				t.Errorf("the ToolSearch answer's tool_reference was dropped: %s", got)
			}
			var m struct {
				Messages []json.RawMessage `json:"messages"`
			}
			json.Unmarshal([]byte(got), &m)
			for _, x := range m.Messages {
				if string(x) == `{"role":"system","content":[]}` || strings.Contains(string(x), `"content":[]`) {
					t.Errorf("a message left empty was sent: %s", x)
				}
			}
		})
	}
}

// DeepSeek's Anthropic endpoint takes a tool_addition naming a tool but
// refuses a tool_removal (422 "unknown variant `tool_removal`"): it is
// asked again at once without the blocks, and from then on sent none. A
// provider that takes them is sent them as Claude Code wrote them.
func TestDeferredToolBlocksRefusedOnceLeftOut(t *testing.T) {
	fresh(t)
	ds := &deferredVendor{deepseek: true}
	dsSrv := httptest.NewServer(ds)
	t.Cleanup(dsSrv.Close)
	takes := &deferredVendor{deepseek: true}
	tkSrv := httptest.NewServer(takes)
	t.Cleanup(tkSrv.Close)
	if err := provider.Save(provider.Provider{ID: "deepseek", Name: "DeepSeek", Preset: "deepseek", Key: "k", Anthropic: dsSrv.URL,
		Models: []string{"deepseek-chat"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "other", Name: "Other", Key: "k", Anthropic: tkSrv.URL,
		Models: []string{"deepseek-chat"}}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	post := func(body string) {
		t.Helper()
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("status %d: %s", rec.Code, rec.Body)
		}
	}
	post(deferredBody("deepseek/deepseek-chat", ccRemove))
	sent := ds.sent()
	if len(sent) != 2 || !strings.Contains(sent[0], `"tool_removal"`) || strings.Contains(sent[1], `"tool_removal"`) {
		t.Fatalf("DeepSeek was asked %d times, want the removal once and then without it: %q", len(sent), sent)
	}
	// learned: the next request goes without them from the start, an
	// addition DeepSeek would take included
	post(deferredBody("deepseek/deepseek-chat", ccAddRef))
	if sent = ds.sent(); len(sent) != 3 || strings.Contains(sent[2], `"tool_addition"`) {
		t.Fatalf("after the refusal DeepSeek was sent %q", sent[2:])
	}
	// a provider that took them keeps getting them
	post(deferredBody("other/deepseek-chat", ccAddRef))
	if got := takes.sent(); len(got) != 1 || !strings.Contains(got[0], `"tool_addition"`) {
		t.Fatalf("a provider that takes tool_addition was sent %q", got)
	}
}
