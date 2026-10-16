package gateway

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/usage"
)

// besideFake installs the fake plugin as the plugin of the built-in id,
// not moved onto it (id-plugin beside the built-in), signed in with a
// key, its requests going to up; it gives the provider's id.
func besideFake(t *testing.T, id string, up http.Handler) string {
	t.Helper()
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("FAKE_ID", id)
	t.Setenv("FAKE_RESPONSES", "1")
	t.Cleanup(plugin.Settle)
	srv := httptest.NewServer(up)
	t.Cleanup(srv.Close)
	t.Setenv("FAKE_BASE", srv.URL+"/v1")
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.APIKey(ctx, id, 0, nil, "k1", plugin.NewAccount); err != nil {
		t.Fatal(err)
	}
	pid := provider.PluginID(id)
	if p, err := provider.Find(pid); err != nil || !p.IsPlugin() || p.PluginProvider() != id {
		t.Fatalf("Find(%s) = %+v, %v", pid, p, err)
	}
	return pid
}

// The Grok plugin draws at Grok's Imagine API as the built-in does
// (TestGrokAccountDraws): the same models, the same request, sent through
// the plugin, which signs it.
func TestGrokPluginDraws(t *testing.T) {
	var mu sync.Mutex
	var paths, bodies []string
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		paths, bodies = append(paths, r.URL.Path), append(bodies, string(b))
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"data":[{"b64_json":"`+base64.StdEncoding.EncodeToString(pngBytes)+`"}]}`)
	}))
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if ds := Drawers(*p); len(ds) != 2 || ds[0].ID != "grok-imagine-image" || ds[0].Provider != pid {
		t.Fatalf("drawers %v", ds)
	}
	if vs := Videomakers(*p); len(vs) == 0 {
		t.Fatal("the Grok plugin makes no videos")
	}
	code, a, raw := postImages(t, New(), "/v1/images/generations", "application/json", `{"model":"`+pid+`/grok-imagine-image","prompt":"a magpie","size":"1792x1024"}`)
	if code != 200 || len(a.Data) != 1 {
		t.Fatalf("%d %s", code, raw)
	}
	// a newer image model by its bare name, as with the built-in (#1857,
	// TestGrokDrawsANewerModelByItsBareName)
	code, a, raw = postImages(t, New(), "/v1/images/generations", "application/json", `{"model":"grok-imagine-image-2.0","prompt":"a magpie"}`)
	if code != 200 || len(a.Data) != 1 {
		t.Fatalf("bare grok-imagine-image-2.0: %d %s", code, raw)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(paths) != 2 || paths[0] != "/v1/images/generations" || !strings.Contains(bodies[0], `"model":"grok-imagine-image"`) || !strings.Contains(bodies[0], `"aspect_ratio":"16:9"`) || !strings.Contains(bodies[1], `"model":"grok-imagine-image-2.0"`) {
		t.Fatalf("asked %v %v", paths, bodies)
	}
}

// Codex's namespaced tools reach the Grok plugin flat, and its call comes
// back under the namespace, as with the built-in (TestGrokGetsNamespacedTools).
// The request asks for one body, which is relayed streamed (plugins#112), so
// the namespace's freeform tool is left out as the built-in leaves it
// (grokBody): no longer offered as the function a translation made of it.
func TestGrokPluginGetsNamespacedTools(t *testing.T) {
	var mu sync.Mutex
	var sent []map[string]any
	call := `{"type":"function_call","id":"fc_1","call_id":"c9","name":"collaboration__spawn_agent","arguments":"{\"message\":\"go\"}","status":"completed"}`
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		done := `{"id":"r1","object":"response","status":"completed","output":[` + call + `],"usage":{"input_tokens":5,"output_tokens":3}}`
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, done)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_item.added`+"\n"+`data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"c9","name":"collaboration__spawn_agent","arguments":""}}`,
			`event: response.output_item.done`+"\n"+`data: {"type":"response.output_item.done","output_index":0,"item":`+call+`}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":`+done+`}`))
	}))
	body := `{"model":"` + pid + `/fake-resp","stream":false,` + namespacedTools + `,"input":[
		{"type":"message","role":"user","content":"go"},
		{"type":"function_call","call_id":"c1","name":"spawn_agent","namespace":"collaboration","arguments":"{}"},
		{"type":"function_call_output","call_id":"c1","output":"ok"}]}`
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	mu.Lock()
	if len(sent) != 1 {
		t.Fatalf("asked %d times", len(sent))
	}
	q := sent[0]
	mu.Unlock()
	var names []string
	for _, tl := range q["tools"].([]any) {
		tm := tl.(map[string]any)
		names = append(names, tm["type"].(string)+":"+tm["name"].(string))
	}
	if got := strings.Join(names, ","); got != "function:exec_command,function:collaboration__spawn_agent" {
		t.Fatalf("Grok was offered %s", got)
	}
	if c := q["input"].([]any)[1].(map[string]any); c["name"] != "collaboration__spawn_agent" || c["namespace"] != nil {
		t.Fatalf("the call handed back went as %v", c)
	}
	var res struct {
		Output []map[string]any `json:"output"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &res); err != nil || len(res.Output) != 1 {
		t.Fatalf("%v: %s", err, rec.Body)
	}
	if it := res.Output[0]; it["name"] != "spawn_agent" || it["namespace"] != "collaboration" {
		t.Fatalf("Codex was given %v", it)
	}
}

// Kiro's plugin beside the built-in (kiro-plugin) counts by estimate, as
// the built-in and the moved plugin do: the vendor isn't asked.
func TestPluginBesideBuiltinCountsByEstimate(t *testing.T) {
	up := &countAsked{}
	pid := besideFake(t, "kiro", up)
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages/count_tokens",
		strings.NewReader(`{"model":"`+pid+`/fake-claude","messages":[{"role":"user","content":"hello there"}]}`)))
	if n := estimated(t, rec); n == 999 || len(up.asked()) > 0 {
		t.Fatalf("the vendor was asked to count: %v, %d", up.asked(), n)
	}
}

// Grok's plugin beside the built-in Grok (grok-plugin) searches by itself
// as the built-in does, on Responses only.
func TestGrokPluginSearchesItself(t *testing.T) {
	pid := besideFake(t, "grok", http.NotFoundHandler())
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if pid != "grok-plugin" || !searchesItself(*p, provider.Responses) || searchesItself(*p, provider.Chat) {
		t.Fatalf("%s searches by itself: %v on Responses, %v on Chat", pid, searchesItself(*p, provider.Responses), searchesItself(*p, provider.Chat))
	}
}

// A Codex subagent's plain agent_message reaches the Grok plugin as the
// user's message, as with the built-in (TestGrokGetsAgentMessageAsUser):
// streamed, as Codex asks, the request is relayed, not translated.
func TestGrokPluginGetsAgentMessageAsUser(t *testing.T) {
	var mu sync.Mutex
	var sent []map[string]any
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		done := `{"id":"r1","object":"response","status":"completed","output":[{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"ok","annotations":[]}]}],"usage":{"input_tokens":5,"output_tokens":1}}`
		if q["stream"] != true {
			w.Header().Set("Content-Type", "application/json")
			io.WriteString(w, done)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":`+done+`}`))
	}))
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(`{"model":"`+pid+`/fake-resp","stream":true,`+agentMessageTurn+`}`)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(sent) != 1 {
		t.Fatalf("asked %d times", len(sent))
	}
	checkAgentMessageSent(t, sent[0])
}

// A Grok model the Grok plugin serves writes Codex's integers as floats
// too; Codex reads them as integers, as from a relay
// (TestGrokCallsGiveCodexIntegers), streamed in deltas or whole.
func TestGrokPluginCallsGiveCodexIntegers(t *testing.T) {
	c := integralCases(t)[0]
	t.Setenv("FAKE_GROK", "1")
	pid := besideFake(t, "grok", responsesCall(c.tool, c.args, someCuts(c.args)["by byte"]))
	for _, stream := range []bool{true, false} {
		checkResponses(t, c, askGrok(t, "/v1/responses", codexShellTurn(pid+"/grok-4.7", stream)), stream)
	}
}

// A non-streaming Responses request to the Grok plugin, an account that
// only streams, is relayed streamed and given back whole (plugins#112): the
// hosted x_search reaches the vendor, and its custom_tool_call items, the
// response's tools and its usage details reach the client. A translation
// dropped all three, so Grok answered without searching.
func TestGrokPluginNonStreamKeepsHostedTools(t *testing.T) {
	var mu sync.Mutex
	var sent []map[string]any
	call := `{"type":"custom_tool_call","id":"ctc_1","call_id":"xs_1","name":"x_keyword_search","input":"{\"query\":\"magpie\"}","status":"completed"}`
	msg := `{"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Found it.","annotations":[]}]}`
	done := `{"id":"r1","object":"response","status":"completed","model":"grok-4.7","tools":[{"type":"x_search"}],"output":[` + call + `,` + msg + `],"usage":{"input_tokens":50,"output_tokens":7,"total_tokens":57,"server_side_tool_usage_details":{"web_search_calls":0,"x_search_calls":1}}}`
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var q map[string]any
		json.Unmarshal(b, &q)
		mu.Lock()
		sent = append(sent, q)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_item.done`+"\n"+`data: {"type":"response.output_item.done","output_index":0,"item":`+call+`}`,
			`event: response.output_item.done`+"\n"+`data: {"type":"response.output_item.done","output_index":1,"item":`+msg+`}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":`+done+`}`))
	}))
	body := `{"model":"` + pid + `/fake-resp","stream":false,"tools":[{"type":"x_search"}],"input":"What do people say about magpie?"}`
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
	if rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	if ct := rec.Header().Get("Content-Type"); ct != "application/json" {
		t.Fatalf("Content-Type %q: %s", ct, rec.Body)
	}
	mu.Lock()
	if len(sent) != 1 {
		t.Fatalf("asked %d times", len(sent))
	}
	q := sent[0]
	mu.Unlock()
	if q["stream"] != true {
		t.Fatalf("the plugin was asked stream=%v", q["stream"])
	}
	if tools, _ := json.Marshal(q["tools"]); string(tools) != `[{"type":"x_search"}]` {
		t.Fatalf("the plugin was offered %s", tools)
	}
	var res struct {
		Object string           `json:"object"`
		Tools  []map[string]any `json:"tools"`
		Output []map[string]any `json:"output"`
		Usage  struct {
			Details map[string]any `json:"server_side_tool_usage_details"`
		} `json:"usage"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &res); err != nil {
		t.Fatalf("%v: %s", err, rec.Body)
	}
	if res.Object != "response" || len(res.Tools) != 1 || res.Tools[0]["type"] != "x_search" {
		t.Fatalf("tools %v: %s", res.Tools, rec.Body)
	}
	if len(res.Output) != 2 || res.Output[0]["type"] != "custom_tool_call" || res.Output[0]["name"] != "x_keyword_search" || res.Output[1]["type"] != "message" {
		t.Fatalf("output %v", res.Output)
	}
	if res.Usage.Details["x_search_calls"] != float64(1) {
		t.Fatalf("usage details %v", res.Usage.Details)
	}
	// and the stream's usage is what the request is counted at
	recs := usage.Load(time.Time{})
	if len(recs) == 0 || recs[len(recs)-1].Input != 50 || recs[len(recs)-1].Output != 7 {
		t.Fatalf("recorded %+v", recs)
	}
}

// A stream that ends before its response.completed is a 502 to a client
// that asked for one body, never an empty 200 (TestNonStreamEndedShort's,
// for the relayed path, plugins#112).
func TestGrokPluginNonStreamEndedShort(t *testing.T) {
	pid := besideFake(t, "grok", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, sse(
			`event: response.created`+"\n"+`data: {"type":"response.created","response":{"id":"r1","object":"response","status":"in_progress","output":[]}}`,
			`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Found"}`))
	}))
	body := `{"model":"` + pid + `/fake-resp","stream":false,"input":"hi"}`
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
	if rec.Code != 502 || !strings.Contains(rec.Body.String(), "ended before it was complete") {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
}

// A stream whose response.completed names no output, and which sent no
// output_item.done either, still gives the client the text it streamed.
func TestGatheredTextOnlyStream(t *testing.T) {
	g := &gathered{h: http.Header{}, status: 200}
	g.h.Set("Content-Type", "text/event-stream")
	io.WriteString(g, sse(
		`data: {"type":"response.created","response":{"id":"r1","model":"m"}}`,
		`data: {"type":"response.output_text.delta","delta":"po"}`,
		`data: {"type":"response.output_text.delta","delta":"ng"}`,
		`data: {"type":"response.completed","response":{"id":"r1","object":"response","status":"completed","usage":{"input_tokens":7,"output_tokens":1}}}`))
	rec := httptest.NewRecorder()
	if code, _ := g.finish(rec, provider.Provider{Name: "p"}, true, "", ""); code != 200 {
		t.Fatalf("%d %s", code, rec.Body)
	}
	var res struct {
		Output []struct {
			Type    string `json:"type"`
			Content []struct {
				Text string `json:"text"`
			} `json:"content"`
		} `json:"output"`
	}
	if json.Unmarshal(rec.Body.Bytes(), &res) != nil || len(res.Output) != 1 || res.Output[0].Type != "message" || len(res.Output[0].Content) != 1 || res.Output[0].Content[0].Text != "pong" {
		t.Fatalf("%s", rec.Body)
	}
}
