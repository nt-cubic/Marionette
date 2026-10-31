package gateway

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/access"
)

// fakeHub is the library's servers, as the gateway asks for them.
type fakeHub struct {
	mu    sync.Mutex
	on    bool
	tools map[string][]HubTool // by agent
	calls []string
	call  func(ctx context.Context, server, tool string) (json.RawMessage, error)
}

func (f *fakeHub) On() bool { f.mu.Lock(); defer f.mu.Unlock(); return f.on }

func (f *fakeHub) Tools(_ context.Context, agent string) []HubTool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.tools[agent]
}

func (f *fakeHub) Call(ctx context.Context, agent, server, tool string, args json.RawMessage) (json.RawMessage, error) {
	f.mu.Lock()
	f.calls = append(f.calls, agent+" "+server+" "+tool+" "+string(args))
	call := f.call
	f.mu.Unlock()
	if call != nil {
		return call(ctx, server, tool)
	}
	return json.RawMessage(`{"content":[{"type":"text","text":"ran ` + server + ` ` + tool + `"}]}`), nil
}

func (f *fakeHub) Stop()  {}
func (f *fakeHub) Prune() {}

func (f *fakeHub) ran() []string {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]string(nil), f.calls...)
}

func hubSpec(name string) map[string]json.RawMessage {
	return map[string]json.RawMessage{
		"name":        json.RawMessage(`"` + name + `"`),
		"description": json.RawMessage(`"Does ` + name + `."`),
		"inputSchema": json.RawMessage(`{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}`),
	}
}

func withHub(t *testing.T, f *fakeHub) {
	t.Helper()
	old := MCPHub
	MCPHub = f
	t.Cleanup(func() { MCPHub = old })
}

func hubPost(h http.Handler, agent, addr, body string, hdr ...string) *httptest.ResponseRecorder {
	return hubPostAt(h, HubMCPPath+agent, addr, body, hdr...)
}

func hubPostAt(h http.Handler, path, addr, body string, hdr ...string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	r.RemoteAddr, r.Host = addr, "127.0.0.1:3425"
	r.Header.Set("Content-Type", "application/json")
	r.Header.Set("Accept", "application/json, text/event-stream")
	for i := 0; i+1 < len(hdr); i += 2 {
		r.Header.Set(hdr[i], hdr[i+1])
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

const here = "127.0.0.1:50000"

// The hub lists the tools of the servers ticked for the agent in its path,
// each <server>__<tool> in the characters a client takes, 64 at most, two
// of one name told apart, and runs each call on the server it is of.
func TestHubMCPListsAndRoutes(t *testing.T) {
	fresh(t)
	long := strings.Repeat("x", 70)
	f := &fakeHub{on: true, tools: map[string][]HubTool{
		"claude": {
			{Server: "github", Tool: "create_issue", Spec: hubSpec("create_issue")},
			{Server: "my server", Tool: "do.it", Spec: hubSpec("do.it")},
			{Server: "my.server", Tool: "do it", Spec: hubSpec("do it")},
			{Server: "fs", Tool: long, Spec: map[string]json.RawMessage{"name": json.RawMessage(`"` + long + `"`)}},
		},
		"codex": {{Server: "fs", Tool: "read", Spec: hubSpec("read")}},
	}}
	withHub(t, f)
	h := lanGuard(New().Handler())

	init := readMCP(t, hubPost(h, "claude", here, `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}`))
	if init.Error != nil || init.Result.ServerInfo.Name != "magpie" || init.Result.ProtocolVersion != "2025-06-18" {
		t.Fatalf("initialize: %+v", init)
	}
	m := readMCP(t, hubPost(h, "claude", here, toolsList))
	var names []string
	for _, x := range m.Result.Tools {
		names = append(names, x.Name)
		if len(x.Name) > 64 {
			t.Errorf("%q is past 64 characters", x.Name)
		}
	}
	want := []string{"github__create_issue", "my-server__do-it", "my-server__do-it_2", "fs__" + strings.Repeat("x", 60)}
	if strings.Join(names, " ") != strings.Join(want, " ") {
		t.Fatalf("tools = %v, want %v", names, want)
	}
	if r := m.Result.Tools[0].InputSchema.Required; len(r) != 1 || r[0] != "q" {
		t.Errorf("schema not passed on: %v", r)
	}
	// a tool listed with no schema is given an empty one
	w := hubPost(h, "claude", here, toolsList)
	if !strings.Contains(w.Body.String(), `"inputSchema":{"type":"object","properties":{}}`) {
		t.Errorf("no default schema: %s", w.Body)
	}

	m = readMCP(t, hubPost(h, "claude", here, toolsCall("my-server__do-it_2", `{"q":"x"}`)))
	if m.Result.IsError || m.Result.Content[0].Text != "ran my.server do it" {
		t.Fatalf("call: %+v", m)
	}
	m = readMCP(t, hubPost(h, "codex", here, `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fs__read"}}`))
	if m.Result.IsError || m.Result.Content[0].Text != "ran fs read" {
		t.Fatalf("codex's call: %+v", m)
	}
	// a tool the agent isn't given is unknown, even another agent's
	for _, c := range []struct{ agent, tool string }{{"codex", "github__create_issue"}, {"claude", "nope__x"}, {"gemini", "fs__read"}} {
		m := readMCP(t, hubPost(h, c.agent, here, toolsCall(c.tool, `{}`)))
		if m.Error == nil || m.Error.Code != -32602 || !strings.Contains(m.Error.Message, "unknown tool "+c.tool) {
			t.Errorf("%s's %s: %+v", c.agent, c.tool, m)
		}
	}
	if got := f.ran(); strings.Join(got, "|") != `claude my.server do it {"q":"x"}|codex fs read {}` {
		t.Errorf("ran %q", got)
	}
	if m := readMCP(t, hubPost(h, "claude", here, `{"jsonrpc":"2.0","id":4,"method":"resources/list"}`)); m.Error == nil || m.Error.Code != -32601 {
		t.Errorf("resources/list: %+v", m)
	}
}

// HubMCPAll, the one address any other app is given, is the hub for agent
// "": every server in the Library, with its own instructions and its own
// word for a tool it doesn't have; an agent's path still lists only its own.
func TestHubMCPForAnyApp(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	f := &fakeHub{on: true, tools: map[string][]HubTool{
		"":       {{Server: "github", Tool: "create_issue", Spec: hubSpec("create_issue")}, {Server: "notes", Tool: "find", Spec: hubSpec("find")}},
		"claude": {{Server: "github", Tool: "create_issue", Spec: hubSpec("create_issue")}},
	}}
	withHub(t, f)
	h := lanGuard(New().Handler())

	w := hubPostAt(h, HubMCPAll, here, `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}`)
	if init := readMCP(t, w); init.Error != nil || init.Result.ServerInfo.Name != "magpie" {
		t.Fatalf("initialize: %+v", init)
	}
	if !strings.Contains(w.Body.String(), "The tools of every MCP server in magpie's Library") {
		t.Errorf("another app's instructions: %s", w.Body)
	}
	var names []string
	for _, x := range readMCP(t, hubPostAt(h, HubMCPAll, here, toolsList)).Result.Tools {
		names = append(names, x.Name)
	}
	if strings.Join(names, " ") != "github__create_issue notes__find" {
		t.Fatalf("another app's tools %v", names)
	}
	m := readMCP(t, hubPostAt(h, HubMCPAll, here, toolsCall("notes__find", `{"q":"x"}`)))
	if m.Result.IsError || m.Result.Content[0].Text != "ran notes find" {
		t.Fatalf("another app's call: %+v", m)
	}
	m = readMCP(t, hubPostAt(h, HubMCPAll, here, toolsCall("nope__x", `{}`)))
	if m.Error == nil || m.Error.Code != -32602 || m.Error.Message != "unknown tool nope__x: no MCP server in magpie's Library gives it" {
		t.Errorf("another app's unknown tool: %+v", m)
	}
	// claude's own path doesn't reach a server it isn't given
	m = readMCP(t, hubPost(h, "claude", here, toolsCall("notes__find", `{}`)))
	if m.Error == nil || !strings.Contains(m.Error.Message, "no MCP server switched on for claude") {
		t.Errorf("claude reached notes: %+v", m)
	}
	if got := f.ran(); strings.Join(got, "|") != ` notes find {"q":"x"}` {
		t.Errorf("ran %q", got)
	}
	// guarded as the agents' paths are: no web page, and no other computer
	// while magpie isn't shared
	if w := hubPostAt(h, HubMCPAll, here, toolsList, "Origin", "https://evil.example", "Sec-Fetch-Site", "cross-site", "Sec-Fetch-Mode", "cors"); w.Code != http.StatusForbidden {
		t.Errorf("a foreign page's tools/list: %d %s", w.Code, w.Body)
	}
	if w := hubPostAt(h, HubMCPAll, "192.0.2.7:4000", toolsCall("notes__find", `{}`)); w.Code != http.StatusForbidden {
		t.Errorf("another computer, not shared: %d %s", w.Code, w.Body)
	}
	if got := f.ran(); len(got) != 1 {
		t.Errorf("ran for a request turned away: %q", got)
	}
}

// A server that fails, or doesn't answer in the hub's time, is an error
// result naming it, never a wait past the client's.
func TestHubMCPServerDown(t *testing.T) {
	fresh(t)
	f := &fakeHub{on: true, tools: map[string][]HubTool{"claude": {
		{Server: "db", Tool: "query", Spec: hubSpec("query")},
		{Server: "slow", Tool: "wait", Spec: hubSpec("wait")},
	}}}
	f.call = func(ctx context.Context, server, tool string) (json.RawMessage, error) {
		if server == "db" {
			return nil, errors.New("it exited (5): Error: lost the database")
		}
		<-ctx.Done()
		return nil, errors.New("it didn't answer in time")
	}
	withHub(t, f)
	wait := hubCallWait
	hubCallWait = 200 * time.Millisecond
	t.Cleanup(func() { hubCallWait = wait })
	h := lanGuard(New().Handler())
	m := readMCP(t, hubPost(h, "claude", here, toolsCall("db__query", `{}`)))
	if !m.Result.IsError || m.Result.Content[0].Text != "The MCP server db couldn't run query: it exited (5): Error: lost the database" {
		t.Errorf("a server down: %+v", m)
	}
	start := time.Now()
	m = readMCP(t, hubPost(h, "claude", here, toolsCall("slow__wait", `{}`)))
	if !m.Result.IsError || m.Result.Content[0].Text != "The MCP server slow couldn't run wait: it didn't answer in 200ms" {
		t.Errorf("a server that doesn't answer: %+v", m)
	}
	if d := time.Since(start); d > 30*time.Second {
		t.Errorf("waited %v", d)
	}
}

// Off, the hub lists no tool and runs none, and says how to turn it on.
func TestHubMCPOff(t *testing.T) {
	fresh(t)
	f := &fakeHub{tools: map[string][]HubTool{"claude": {{Server: "fs", Tool: "read", Spec: hubSpec("read")}}}}
	withHub(t, f)
	h := lanGuard(New().Handler())
	if m := readMCP(t, hubPost(h, "claude", here, toolsList)); len(m.Result.Tools) != 0 || m.Error != nil {
		t.Errorf("off, listed %+v", m)
	}
	m := readMCP(t, hubPost(h, "claude", here, toolsCall("fs__read", `{}`)))
	if !m.Result.IsError || !strings.Contains(m.Result.Content[0].Text, "MCP hub is off") {
		t.Errorf("off, a call said %+v", m)
	}
	if len(f.ran()) != 0 {
		t.Errorf("off, ran %q", f.ran())
	}
	MCPHub = nil
	if m := readMCP(t, hubPost(h, "claude", here, toolsList)); len(m.Result.Tools) != 0 || m.Error != nil {
		t.Errorf("no hub, listed %+v", m)
	}
}

// The hub is guarded as magpie's other MCP servers are: another computer
// only while shared and with a key, and no web page — a sandboxed frame's
// null Origin, another site's, one on this computer, one rebound to
// 127.0.0.1 — even from this computer. No tool runs for one turned away.
func TestHubMCPGuarded(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	f := &fakeHub{on: true, tools: map[string][]HubTool{"claude": {{Server: "fs", Tool: "read", Spec: hubSpec("read")}}}}
	withHub(t, f)
	h := lanGuard(New().Handler())
	call := toolsCall("fs__read", `{"q":"x"}`)
	const there = "192.0.2.7:4000"
	if w := hubPost(h, "claude", there, call); w.Code != http.StatusForbidden {
		t.Fatalf("not shared: %d %s", w.Code, w.Body)
	}
	_, secrets := newCaller(t, "Laptop") // shares magpie
	if w := hubPost(h, "claude", there, call); w.Code != http.StatusUnauthorized {
		t.Fatalf("shared, no key: %d %s", w.Code, w.Body)
	}
	if w := hubPost(h, "claude", there, call, "Authorization", "Bearer sk-magpie-wrong"); w.Code != http.StatusUnauthorized {
		t.Fatalf("shared, a wrong key: %d %s", w.Code, w.Body)
	}
	for _, hdr := range [][]string{
		{"Origin", "null", "Sec-Fetch-Mode", "cors"},
		{"Origin", "https://evil.example", "Sec-Fetch-Site", "cross-site", "Sec-Fetch-Mode", "cors"},
		{"Origin", "http://localhost:5173", "Sec-Fetch-Site", "same-site", "Sec-Fetch-Mode", "cors"},
	} {
		if w := hubPost(h, "claude", here, call, hdr...); w.Code != http.StatusForbidden {
			t.Errorf("a page %v: %d %s", hdr, w.Code, w.Body)
		}
	}
	r := httptest.NewRequest(http.MethodPost, HubMCPPath+"claude", strings.NewReader(call))
	r.RemoteAddr, r.Host = here, "rebind.attacker.example:3425"
	r.Header.Set("Origin", "http://rebind.attacker.example:3425")
	r.Header.Set("Sec-Fetch-Site", "same-origin")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, r)
	if rec.Code != http.StatusForbidden {
		t.Errorf("a rebound page: %d %s", rec.Code, rec.Body)
	}
	if w := httptest.NewRecorder(); true {
		h.ServeHTTP(w, httptest.NewRequest(http.MethodGet, HubMCPPath+"claude", nil))
		if w.Code == http.StatusOK {
			t.Errorf("a GET: %d", w.Code)
		}
	}
	if got := f.ran(); len(got) != 0 {
		t.Fatalf("ran for a request turned away: %q", got)
	}
	m := readMCP(t, hubPost(h, "claude", there, call, "Authorization", "Bearer "+secrets[0]))
	if m.Result.IsError || m.Result.Content[0].Text != "ran fs read" {
		t.Fatalf("shared, with its key: %+v", m)
	}
	m = readMCP(t, hubPost(h, "claude", here, call))
	if m.Result.IsError || len(f.ran()) != 2 {
		t.Fatalf("this computer: %+v, ran %q", m, f.ran())
	}
}

// A gateway key's limit holds each call: a key held to a limit nothing
// serves is refused, and the server isn't asked.
func TestHubMCPHeldToKeyLimit(t *testing.T) {
	fresh(t)
	f := &fakeHub{on: true, tools: map[string][]HubTool{"claude": {{Server: "fs", Tool: "read", Spec: hubSpec("read")}}}}
	withHub(t, f)
	withoutLimits(t)
	keys, _ := newCaller(t, "Capped")
	setLimit(t, keys[0].ID, &access.Limit{Period: "day", Tokens: 700})
	capped := access.Identity{KeyID: keys[0].ID, KeyName: "Capped", Limit: &access.Limit{Period: "day", Tokens: 700}}
	s := New()
	r := httptest.NewRequest("POST", HubMCPPath+"claude", nil).WithContext(access.WithIdentity(t.Context(), capped))
	res, rpcErr := s.hubMCPCall(r, "claude", mcpMessage{ID: json.RawMessage("1"), Method: "tools/call", Params: json.RawMessage(`{"name":"fs__read","arguments":{}}`)})
	b, _ := json.Marshal(res)
	if rpcErr != nil || !strings.Contains(string(b), `"isError":true`) || !strings.Contains(string(b), "Key limits plugin") {
		t.Errorf("a limited key with limits off: %s %v", b, rpcErr)
	}
	if len(f.ran()) != 0 {
		t.Errorf("ran %q", f.ran())
	}
}
