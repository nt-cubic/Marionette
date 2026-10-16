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

// openDesignVendor answers as OpenDesign's Anthropic endpoint answered
// #2124's reporter: a tool_result whose content is tool_reference blocks
// alone is a 422 "missing field `content`" (its index counting something
// other than the messages sent), and the same result with a text block
// beside the references is taken. takes is a vendor that reads either.
type openDesignVendor struct {
	takes  bool
	mu     sync.Mutex
	paths  []string
	bodies []string
}

func (v *openDesignVendor) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	v.mu.Lock()
	v.paths = append(v.paths, r.URL.Path)
	v.bodies = append(v.bodies, string(b))
	v.mu.Unlock()
	var m struct {
		Messages []struct {
			Content json.RawMessage `json:"content"`
		} `json:"messages"`
	}
	json.Unmarshal(b, &m)
	refused := false
	for _, msg := range m.Messages {
		var blocks []struct {
			Type    string `json:"type"`
			Content []struct {
				Type string `json:"type"`
			} `json:"content"`
		}
		json.Unmarshal(msg.Content, &blocks)
		for _, bl := range blocks {
			if bl.Type != "tool_result" || len(bl.Content) == 0 {
				continue
			}
			text := false
			for _, c := range bl.Content {
				text = text || c.Type == "text"
			}
			refused = refused || !text && !v.takes
		}
	}
	w.Header().Set("Content-Type", "application/json")
	switch {
	case refused:
		w.WriteHeader(422)
		io.WriteString(w, "Failed to deserialize the JSON body into the target type: messages[4]: missing field `content` at line 38 column 3")
	case strings.HasSuffix(r.URL.Path, "/count_tokens"):
		io.WriteString(w, `{"input_tokens":12}`)
	default:
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-v4.1-flash","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}`)
	}
}

func (v *openDesignVendor) sent() []string {
	v.mu.Lock()
	defer v.mu.Unlock()
	return append([]string(nil), v.bodies...)
}

// #2124's repro.json, as the reporter posted it, with magpie's model.
func reporterRefsOnly(model string) string {
	return `{
  "model": "` + model + `",
  "max_tokens": 64,
  "thinking": {"type": "disabled"},
  "system": "This is a synthetic format test. Reply OK. Do not call any tool.",
  "tools": [
    {
      "name": "ToolSearch",
      "description": "Find a test tool.",
      "input_schema": {
        "type": "object",
        "properties": {"query": {"type": "string"}},
        "required": ["query"]
      }
    },
    {
      "name": "check",
      "description": "Return a fixed synthetic result.",
      "input_schema": {"type": "object", "properties": {}}
    }
  ],
  "messages": [
    {"role": "user", "content": "Reply with OK only."},
    {
      "role": "assistant",
      "content": [
        {"type": "text", "text": "I will find the check tool."},
        {
          "type": "tool_use",
          "id": "toolu_direct_probe",
          "name": "ToolSearch",
          "input": {"query": "check"}
        }
      ]
    },
    {
      "role": "user",
      "content": [
        {
          "type": "tool_result",
          "tool_use_id": "toolu_direct_probe",
          "content": [{"type": "tool_reference", "tool_name": "check"}]
        }
      ]
    }
  ]
}`
}

// #2124 (liuyejinghong): OpenDesign, added as a custom Anthropic provider,
// refused every turn after ToolSearch answered with tool_reference blocks
// alone. It is asked again at once with a text block naming the loaded
// tools before the references, and from then on sent them that way, on
// /v1/messages, count_tokens and through a group. The references and the
// tool_use pairing stay. A provider that takes the bare references is sent
// them as Claude Code wrote them.
func TestRefsOnlyToolResultRefusedGetsText(t *testing.T) {
	fresh(t)
	od := &openDesignVendor{}
	odSrv := httptest.NewServer(od)
	t.Cleanup(odSrv.Close)
	takes := &openDesignVendor{takes: true}
	tkSrv := httptest.NewServer(takes)
	t.Cleanup(tkSrv.Close)
	if err := provider.Save(provider.Provider{ID: "opendesign", Name: "OpenDesign", Key: "k", Anthropic: odSrv.URL,
		Models: []string{"deepseek-v4.1-flash"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "other", Name: "Other", Key: "k", Anthropic: tkSrv.URL,
		Models: []string{"deepseek-v4.1-flash"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"opendesign/deepseek-v4.1-flash"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	post := func(path, body string) {
		t.Helper()
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("%s: status %d: %s", path, rec.Code, rec.Body)
		}
	}
	said := func(t *testing.T, got string) {
		t.Helper()
		var m struct {
			Messages []struct {
				Content json.RawMessage `json:"content"`
			} `json:"messages"`
		}
		if err := json.Unmarshal([]byte(got), &m); err != nil || len(m.Messages) != 3 {
			t.Fatalf("sent %d messages (%v): %s", len(m.Messages), err, got)
		}
		var res []struct {
			Type      string `json:"type"`
			ToolUseID string `json:"tool_use_id"`
			Content   []struct {
				Type     string `json:"type"`
				Text     string `json:"text"`
				ToolName string `json:"tool_name"`
			} `json:"content"`
		}
		json.Unmarshal(m.Messages[2].Content, &res)
		if len(res) != 1 || res[0].Type != "tool_result" || res[0].ToolUseID != "toolu_direct_probe" || len(res[0].Content) != 2 ||
			res[0].Content[0].Type != "text" || res[0].Content[0].Text != "Tool check is loaded and can be called now." ||
			res[0].Content[1].Type != "tool_reference" || res[0].Content[1].ToolName != "check" {
			t.Fatalf("the ToolSearch answer went as %s", m.Messages[2].Content)
		}
		if !strings.Contains(string(m.Messages[1].Content), `"id": "toolu_direct_probe"`) && !strings.Contains(string(m.Messages[1].Content), `"id":"toolu_direct_probe"`) {
			t.Fatalf("the tool_use was changed: %s", m.Messages[1].Content)
		}
	}

	post("/v1/messages", reporterRefsOnly("opendesign/deepseek-v4.1-flash:max"))
	sent := od.sent()
	if len(sent) != 2 || strings.Contains(sent[0], "is loaded and can be called") {
		t.Fatalf("OpenDesign was asked %d times, want the bare references once and then with text: %q", len(sent), sent)
	}
	said(t, sent[1])

	// learned: the group's member, and counting tokens, go with the text
	// from the start
	post("/v1/messages", reporterRefsOnly("group/g"))
	post("/v1/messages/count_tokens", reporterRefsOnly("opendesign/deepseek-v4.1-flash"))
	sent = od.sent()
	if len(sent) != 4 {
		t.Fatalf("after the refusal OpenDesign was asked %d times, want 4: %q", len(sent), sent)
	}
	said(t, sent[2])
	said(t, sent[3])

	// a provider that reads the bare references keeps getting them
	post("/v1/messages", reporterRefsOnly("other/deepseek-v4.1-flash"))
	if got := takes.sent(); len(got) != 1 || strings.Contains(got[0], "is loaded and can be called") || !strings.Contains(got[0], `"tool_name": "check"`) && !strings.Contains(got[0], `"tool_name":"check"`) {
		t.Fatalf("a provider that takes the references was sent %q", got)
	}
}

// A tool_result that already has text beside its references, or no
// references, is sent as it was, and so is a body without any.
func TestRefsSaidLeavesOtherResults(t *testing.T) {
	for _, body := range []string{
		`{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":[{"type":"text","text":"x"},{"type":"tool_reference","tool_name":"check"}]}]}]}`,
		`{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"done"}]}]}`,
		`{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":[]}]}]}`,
		`{"messages":[{"role":"user","content":"hi"}]}`,
	} {
		if got := string(withRefsSaid([]byte(body))); got != body {
			t.Errorf("%s\nwent as %s", body, got)
		}
	}
	two := `{"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":[{"type":"tool_reference","tool_name":"WebFetch"},{"type":"tool_reference","tool_name":"WebSearch"}]}]}]}`
	if got := string(withRefsSaid([]byte(two))); !strings.Contains(got, `"text":"Tool WebFetch is loaded and can be called now.\nTool WebSearch is loaded and can be called now."`) ||
		!strings.Contains(got, `{"type":"tool_reference","tool_name":"WebFetch"},{"type":"tool_reference","tool_name":"WebSearch"}`) {
		t.Errorf("two references went as %s", got)
	}
}
