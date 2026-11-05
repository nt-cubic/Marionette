package gateway

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// liteRequest is a request as Codex's Responses Lite sends it: no tools and
// no instructions, its tools as the first input item (its own functions in
// the "functions" namespace), its prompt as a developer message, and a call
// made earlier under that namespace.
const liteRequest = `{"model":"fake/m1","instructions":"","stream":true,"input":[
	{"type":"additional_tools","id":"at_1","role":"developer","tools":[
		{"type":"tool_search","execution":"client","description":"Search tools","parameters":{"type":"object"}},
		{"type":"namespace","name":"functions","description":"Default tools","tools":[
			{"type":"function","name":"shell","description":"Run a command","strict":false,"parameters":{"type":"object","properties":{"cmd":{"type":"string"}}}},
			{"type":"custom","name":"apply_patch","description":"Patch files","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}]},
		{"type":"namespace","name":"mcp__probe","description":"Probe","tools":[{"type":"function","name":"read","parameters":{"type":"object"}}]}]},
	{"type":"message","id":"msg_1","role":"developer","content":[{"type":"input_text","text":"You are Codex."}]},
	{"type":"message","role":"user","content":[{"type":"input_text","text":"list the files"}]},
	{"type":"function_call","call_id":"c1","name":"shell","namespace":"functions","arguments":"{\"cmd\":\"ls\"}"},
	{"type":"function_call_output","call_id":"c1","output":"a.txt"}
]}`

// A Responses API other than OpenAI's gets Codex's Lite tools in its tools,
// not as an input item it turns the whole request away over (#350).
func TestAdditionalToolsRelayed(t *testing.T) {
	f := &fake{t: t, reply: sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`data: {"type":"response.completed","response":{"id":"r1","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`)}
	f.refuse = func(body []byte) (int, string) {
		if strings.Contains(string(body), `"additional_tools"`) {
			return http.StatusUnprocessableEntity, `{"error":"Failed to deserialize the JSON body into the target type: input[0]: unknown item type \"additional_tools\""}`
		}
		return 0, ""
	}
	setup(t, provider.Responses, f)
	if code, body := post(t, "/v1/responses", liteRequest); code != 200 {
		t.Fatalf("status %d: %s", code, body)
	}
	var got struct {
		Input []map[string]any `json:"input"`
		Tools []map[string]any `json:"tools"`
	}
	if err := json.Unmarshal(f.got, &got); err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, tl := range got.Tools {
		n, _ := tl["name"].(string)
		names = append(names, tl["type"].(string)+":"+n)
	}
	// tool search as the function magpie serves it; Codex's own tools out
	// of their namespace; an MCP server's namespace as it was
	if want := "function:tool_search function:shell custom:apply_patch namespace:mcp__probe"; strings.Join(names, " ") != want {
		t.Fatalf("tools %v, want %s\n%s", names, want, f.got)
	}
	if len(got.Input) != 4 || got.Input[0]["role"] != "developer" || got.Input[1]["role"] != "user" {
		t.Fatalf("input: %s", f.got)
	}
	if call := got.Input[2]; call["name"] != "shell" || call["namespace"] != nil {
		t.Fatalf("earlier call: %v", call)
	}
}

// A request translated for another API offers the Lite tools as the tools
// they are, and an earlier call to one under its own name.
func TestAdditionalToolsTranslated(t *testing.T) {
	r, err := parseResponses([]byte(liteRequest))
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, tl := range r.Tools {
		names = append(names, tl.Name)
	}
	// apply_patch, a custom tool, is offered as a function taking its input
	if want := "tool_search shell apply_patch mcp__probe__read"; strings.Join(names, " ") != want {
		t.Fatalf("tools %v, want %s", names, want)
	}
	if !r.Namespaced["apply_patch"].Custom {
		t.Fatalf("apply_patch not custom: %+v", r.Namespaced)
	}
	if r.Namespaced["mcp__probe__read"] != (nsTool{Namespace: "mcp__probe", Name: "read"}) {
		t.Fatalf("namespace lost: %+v", r.Namespaced)
	}
	if r.System != "You are Codex." {
		t.Fatalf("system %q", r.System)
	}
	var call *Part
	for _, m := range r.Messages {
		for i, p := range m.Parts {
			if p.Kind == ToolCall {
				call = &m.Parts[i]
			}
		}
	}
	if call == nil || call.Name != "shell" {
		t.Fatalf("earlier call: %+v", call)
	}
}

// liteSent is a request POSTed as Codex sends a Lite model's: the Lite
// header on, and the reasoning context the ChatGPT backend asks of a Lite
// request (#534).
func liteSent(body string) *http.Request {
	req := httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body))
	req.Header.Set("User-Agent", "codex/0.165.0 (Mac OS 27.2; arm64) UnknownTerminal")
	req.Header.Set("originator", "codex_cli_rs")
	req.Header.Set(liteHeader, "true")
	return req
}

// A remote magpie's codex model is served there through its ChatGPT
// account, which passes on what it got, and the ChatGPT backend refuses a
// request for one of its Lite models without reasoning context "all_turns"
// (#4445). So a Lite request routed to it goes as Codex sent it — what the
// shape lift takes off for anyone else leaves the account a request its
// backend turns away.
func TestRemoteMagpieCodexModelKeepsLite(t *testing.T) {
	f := &fake{t: t, reply: sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`data: {"type":"response.completed","response":{"id":"r1","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`)}
	f.refuse = func(body []byte) (int, string) {
		var q struct {
			Reasoning struct {
				Context string `json:"context"`
			} `json:"reasoning"`
		}
		json.Unmarshal(body, &q)
		if q.Reasoning.Context != "all_turns" {
			return http.StatusBadRequest, `{"error":{"message":"X-OpenAI-Internal-Codex-Responses-Lite requires ` + "`reasoning.context`" + ` to be ` + "`all_turns`" + `.","type":"invalid_request_error","code":"unsupported_value","param":"reasoning.context"}}`
		}
		return 0, ""
	}
	fresh(t)
	up := httptest.NewServer(f)
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "sk-office", Responses: up.URL, Models: []string{"codex/m1"}}); err != nil {
		t.Fatal(err)
	}
	body := strings.Replace(liteRequest, `"model":"fake/m1",`,
		`"model":"office/codex/m1","reasoning":{"effort":"xhigh","context":"all_turns"},`, 1)
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, liteSent(body))
	if rec.Code != 200 {
		t.Fatalf("status %d: %s", rec.Code, rec.Body.String())
	}
	var got struct {
		Reasoning map[string]any   `json:"reasoning"`
		Input     []map[string]any `json:"input"`
	}
	if err := json.Unmarshal(f.got, &got); err != nil {
		t.Fatal(err)
	}
	if got.Reasoning["context"] != "all_turns" {
		t.Fatalf("reasoning.context not kept: %s", f.got)
	}
	if f.head.Get(liteHeader) != "true" {
		t.Fatalf("the Lite header was dropped: %v", f.head)
	}
	if len(got.Input) == 0 || got.Input[0]["type"] != "additional_tools" {
		t.Fatalf("the Lite tools were lifted: %s", f.got)
	}
}

// The reporter's own route (#4445): Codex Desktop's turn on one of its
// own models comes to magpie's ChatGPT backend route, and the Model Map
// plugin names the remote magpie's codex model in it.
func TestRemoteMagpieCodexModelKeepsLiteOnCodexPath(t *testing.T) {
	f := &fake{t: t, reply: sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`data: {"type":"response.completed","response":{"id":"r1","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`)}
	f.refuse = func(body []byte) (int, string) {
		var q struct {
			Reasoning struct {
				Context string `json:"context"`
			} `json:"reasoning"`
		}
		json.Unmarshal(body, &q)
		if q.Reasoning.Context != "all_turns" {
			return http.StatusBadRequest, `{"error":{"message":"X-OpenAI-Internal-Codex-Responses-Lite requires ` + "`reasoning.context`" + ` to be ` + "`all_turns`" + `.","type":"invalid_request_error","code":"unsupported_value","param":"reasoning.context"}}`
		}
		return 0, ""
	}
	fresh(t)
	up := httptest.NewServer(f)
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "sk-office", Responses: up.URL, Models: []string{"codex/m1"}}); err != nil {
		t.Fatal(err)
	}
	body := strings.Replace(liteRequest, `"model":"fake/m1",`,
		`"model":"office/codex/m1","reasoning":{"effort":"xhigh","context":"all_turns"},`, 1)
	rec := httptest.NewRecorder()
	req := liteSent(body)
	req.URL.Path = CodexPath + "/responses"
	req.RequestURI = ""
	req.RemoteAddr = "127.0.0.1:51000"
	New().Handler().ServeHTTP(rec, req)
	if rec.Code != 200 {
		t.Fatalf("status %d: %s", rec.Code, rec.Body.String())
	}
	var got struct {
		Reasoning map[string]any   `json:"reasoning"`
		Input     []map[string]any `json:"input"`
	}
	if err := json.Unmarshal(f.got, &got); err != nil {
		t.Fatal(err)
	}
	if got.Reasoning["context"] != "all_turns" {
		t.Fatalf("reasoning.context not kept: %s", f.got)
	}
	if f.head.Get(liteHeader) != "true" {
		t.Fatalf("the Lite header was dropped: %v", f.head)
	}
	if len(got.Input) == 0 || got.Input[0]["type"] != "additional_tools" {
		t.Fatalf("the Lite tools were lifted: %s", f.got)
	}
}

// Another of the remote magpie's models still gets what Codex sends a model
// that isn't Lite (#350): its tools lifted, the reasoning context and the
// Lite header taken off. Only the codex model keeps the shape.
func TestRemoteMagpieOtherModelLeavesLite(t *testing.T) {
	f := &fake{t: t, reply: sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m1"}}`,
		`data: {"type":"response.completed","response":{"id":"r1","output":[],"usage":{"input_tokens":5,"output_tokens":2}}}`)}
	fresh(t)
	up := httptest.NewServer(f)
	t.Cleanup(up.Close)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: "sk-office", Responses: up.URL, Models: []string{"vendor/m"}}); err != nil {
		t.Fatal(err)
	}
	body := strings.Replace(liteRequest, `"model":"fake/m1",`,
		`"model":"office/vendor/m","reasoning":{"effort":"xhigh","context":"all_turns"},`, 1)
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, liteSent(body))
	if rec.Code != 200 {
		t.Fatalf("status %d: %s", rec.Code, rec.Body.String())
	}
	var got struct {
		Reasoning map[string]any   `json:"reasoning"`
		Input     []map[string]any `json:"input"`
	}
	if err := json.Unmarshal(f.got, &got); err != nil {
		t.Fatal(err)
	}
	if _, has := got.Reasoning["context"]; has {
		t.Fatalf("reasoning.context kept on a plain model: %s", f.got)
	}
	if f.head.Get(liteHeader) != "" {
		t.Fatalf("the Lite header was kept on a plain model: %v", f.head)
	}
	if len(got.Input) == 0 || got.Input[0]["type"] == "additional_tools" {
		t.Fatalf("the Lite tools were not lifted: %s", f.got)
	}
}

// Lite tools sent beside tools already there are added once, a namespace
// given again adding the tools it didn't have; an empty additional_tools
// just goes; a request without one is left as it was.
func TestAdditionalToolsMerged(t *testing.T) {
	in := `{"model":"m","tools":[{"type":"function","name":"shell","parameters":{"type":"object"}},
		{"type":"namespace","name":"mcp__probe","tools":[{"type":"function","name":"write"}]}],
		"input":[{"type":"additional_tools","tools":[
			{"type":"namespace","name":"functions","tools":[{"type":"function","name":"shell","description":"again"}]},
			{"type":"namespace","name":"mcp__probe","tools":[{"type":"function","name":"read"},{"type":"function","name":"write"}]}]},
		{"type":"message","role":"user","content":"hi"}]}`
	var got struct {
		Tools []rTool          `json:"tools"`
		Input []map[string]any `json:"input"`
	}
	out := liftAdditionalTools([]byte(in))
	if err := json.Unmarshal(out, &got); err != nil {
		t.Fatal(err)
	}
	if len(got.Tools) != 2 || got.Tools[0].Description != "" || len(got.Tools[1].Tools) != 2 || got.Tools[1].Tools[1].Name != "read" || len(got.Input) != 1 {
		t.Fatalf("merged: %s", out)
	}
	if again := liftAdditionalTools(out); string(again) != string(out) {
		t.Fatalf("lifted twice: %s", again)
	}

	out = liftAdditionalTools([]byte(`{"model":"m","input":[{"type":"additional_tools","tools":[]},{"type":"message","role":"user","content":"hi"}]}`))
	if strings.Contains(string(out), "additional_tools") || strings.Contains(string(out), `"tools"`) || !strings.Contains(string(out), `"content":"hi"`) {
		t.Fatalf("empty: %s", out)
	}
	plain := `{"model":"m","input":"what are additional_tools?"}`
	if out := liftAdditionalTools([]byte(plain)); string(out) != plain {
		t.Fatalf("plain: %s", out)
	}
}
