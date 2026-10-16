package gui

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
)

// ctxRoute is a done request of agent in session whose prompt holds base
// (system prompt, tools, memory) and chat, the cache read of it counted.
func ctxRoute(id int64, agent, session string, base, chat, window, cached int) gateway.Route {
	return gateway.Route{
		ID: id, Time: time.Unix(1_790_000_000+id*60, 0), Agent: agent, Session: session, Model: agent + "-model", Done: true, Status: 200,
		Usage: []gateway.RouteUsage{{CacheRead: cached}},
		Prompt: &gateway.Prompt{Window: window, Tokens: base + chat, Counted: true, Parts: []gateway.PromptPart{
			{Kind: gateway.PartSystem, Tokens: base / 2},
			{Kind: gateway.PartTools, Tokens: base - base/2, Items: []gateway.PromptItem{{Name: "linear", Tag: "mcp", Tokens: base * 2 / 5}}},
			{Kind: gateway.PartChat, Tokens: chat},
		}},
	}
}

func tagKeys(a ctxAgent) []string {
	var keys []string
	for _, tg := range a.Tags {
		keys = append(keys, tg.Key)
	}
	return keys
}

// An agent that starts small, grows slowly and is read from the cache
// scores high and is told lean and cache-friendly; its session's line is
// its requests', oldest first.
func TestContextOfALeanAgent(t *testing.T) {
	var routes []gateway.Route
	for i, chat := range []int{1000, 3000, 6000, 8000} {
		total := 12000 + chat
		routes = append(routes, ctxRoute(int64(i+1), "codex", "s1", 12000, chat, 272000, total*9/10))
	}
	// a title call is no part of what its prompts are like
	title := ctxRoute(9, "codex", "s1", 500, 0, 272000, 0)
	title.Kind = "title"
	routes = append(routes, title)

	out := contextOf(routes, 7)
	if len(out.Agents) != 1 || len(out.Sessions) != 1 {
		t.Fatalf("agents %d sessions %d", len(out.Agents), len(out.Sessions))
	}
	a := out.Agents[0]
	if a.Requests != 4 || a.Baseline != 12000 || a.Window != 272000 {
		t.Fatalf("requests %d baseline %d window %d", a.Requests, a.Baseline, a.Window)
	}
	if a.Score < 90 {
		t.Fatalf("score %d %+v", a.Score, a.Scores)
	}
	for _, want := range []string{"lean", "cache-friendly"} {
		if !slices.Contains(tagKeys(a), want) {
			t.Fatalf("tags %v, want %s", tagKeys(a), want)
		}
	}
	s := out.Sessions[0]
	if s.Requests != 4 || s.Peak != 20000 || s.LatestID != 4 || len(s.Points) != 4 || s.Points[0].Tokens != 13000 {
		t.Fatalf("session %+v", s)
	}
}

// Claude Code's auto mode asks its classifier about each action under the
// conversation's own session id: a security monitor's system prompt and
// the transcript, no tools, 34–48K. Interleaved with turns of 60–116K, as
// Player's glm-5.3 session on Discord was, those drops are no compaction,
// nor are its title, quota check or a subagent's turns; the
// conversation's line, latest request and peak are its own turns', and a
// real compaction of them still counts.
func TestContextSideCallsAreNoCompaction(t *testing.T) {
	const window = 1_000_000
	var routes []gateway.Route
	id := int64(0)
	turn := func(chat int) {
		id++
		routes = append(routes, ctxRoute(id, "claude", "cc-1", 24000, chat, window, 24000+chat*9/10))
	}
	// a request with no tool definitions, as the classifier, the title and
	// the quota check send
	side := func(system, chat int) {
		id++
		r := ctxRoute(id, "claude", "cc-1", 0, chat, window, 0)
		r.Prompt.Tokens = system + chat
		r.Prompt.Parts = []gateway.PromptPart{{Kind: gateway.PartSystem, Tokens: system}, {Kind: gateway.PartChat, Tokens: chat}}
		routes = append(routes, r)
	}
	side(60, 900) // the title, before the first turn has an answer
	side(0, 8)    // the quota check
	chats := []int{36000, 48000, 60000, 72000, 80000, 86000, 92000}
	for i, chat := range chats {
		turn(chat)
		// the classifier on the turn's actions: its transcript grows as the
		// conversation does, well under its turns
		side(6000, 28000+i*2000)
		side(6000, 28000+i*2000+500)
	}
	// a subagent's turns, with tools of their own, under the same session
	for _, chat := range []int{4000, 9000} {
		id++
		r := ctxRoute(id, "claude", "cc-1", 18000, chat, window, 0)
		r.Sub = "agent-a1b2"
		routes = append(routes, r)
	}
	turn(chats[len(chats)-1] + 4000)
	side(6000, 42000)
	// a real compaction: the next turn starts over from the summary
	turn(9000)
	side(6000, 12000)
	turn(14000)
	last := id

	out := contextOf(routes, 7)
	if len(out.Agents) != 1 || len(out.Sessions) != 1 {
		t.Fatalf("agents %d sessions %d", len(out.Agents), len(out.Sessions))
	}
	s, a := out.Sessions[0], out.Agents[0]
	if s.Compacts != 1 || a.Compacts != 1 {
		t.Fatalf("compactions: session %d agent %d, want the one real one", s.Compacts, a.Compacts)
	}
	if want := len(chats) + 3; s.Requests != want || len(s.Points) != want {
		t.Fatalf("session requests %d points %d, want its %d turns", s.Requests, len(s.Points), want)
	}
	if s.LatestID != last || s.Latest.Tokens != 24000+14000 {
		t.Fatalf("latest %d (%d tokens), want turn %d", s.LatestID, s.Latest.Tokens, last)
	}
	if s.Peak != 24000+96000 {
		t.Fatalf("peak %d", s.Peak)
	}
	if a.Baseline != 24000 {
		t.Fatalf("baseline %d: side calls are no part of what its turns start with", a.Baseline)
	}
}

// An agent that sends no tools at all keeps every request on its line.
func TestContextToollessAgentKeepsItsTurns(t *testing.T) {
	var routes []gateway.Route
	for i, chat := range []int{30000, 50000, 70000, 20000} {
		r := ctxRoute(int64(i+1), "chatbox", "c1", 0, chat, 128000, 0)
		r.Prompt.Parts = []gateway.PromptPart{{Kind: gateway.PartChat, Tokens: chat}}
		routes = append(routes, r)
	}
	s := contextOf(routes, 7).Sessions[0]
	if s.Requests != 4 || s.Compacts != 1 {
		t.Fatalf("requests %d compacts %d", s.Requests, s.Compacts)
	}
}

// One that starts with most of a window of MCP tools, misses the cache,
// compacts and fails often scores low and is told so.
func TestContextOfAHeavyAgent(t *testing.T) {
	var routes []gateway.Route
	chats := []int{10000, 40000, 100000, 5000, 20000, 30000}
	for i, chat := range chats {
		routes = append(routes, ctxRoute(int64(i+1), "claude", "s1", 70000, chat, 200000, 1000))
	}
	for i := range 3 {
		routes = append(routes, gateway.Route{ID: int64(20 + i), Time: time.Unix(1_790_000_000, 0), Agent: "claude", Done: true, Status: 529})
	}
	a := contextOf(routes, 7).Agents[0]
	if a.Compacts != 1 || a.Errors != 3 || a.Calls != 9 {
		t.Fatalf("compacts %d errors %d calls %d", a.Compacts, a.Errors, a.Calls)
	}
	if a.Score >= 50 {
		t.Fatalf("score %d %+v", a.Score, a.Scores)
	}
	for _, want := range []string{"cache-misses", "heavy-start", "mcp-heavy", "compacts", "flaky", "near-full"} {
		if !slices.Contains(tagKeys(a), want) {
			t.Fatalf("tags %v, want %s", tagKeys(a), want)
		}
	}
}

// An agent whose vendor doesn't count the cache is scored neither up nor
// down for it, and not told cache-friendly or cache-misses.
func TestContextCacheNotKnown(t *testing.T) {
	var routes []gateway.Route
	for i := range 6 {
		r := ctxRoute(int64(i+1), "gemini", "s1", 8000, 1000*i, 1000000, 0)
		r.Prompt.Counted, r.Usage = false, nil
		routes = append(routes, r)
	}
	a := contextOf(routes, 1).Agents[0]
	if a.Scores[0].Key != "cache" || a.Scores[0].Points != 18 {
		t.Fatalf("cache score %+v", a.Scores[0])
	}
	for _, k := range tagKeys(a) {
		if k == "cache-friendly" || k == "cache-misses" {
			t.Fatalf("tags %v", tagKeys(a))
		}
	}
}

// A session goes by the name the Sessions page shows for it: here the one
// a Claude Code session was renamed to.
func TestContextSessionTitles(t *testing.T) {
	h := sandboxHome(t)
	claude := filepath.Join(h, ".claude")
	proj := filepath.Join(claude, "projects", "-work")
	if err := os.MkdirAll(proj, 0o755); err != nil {
		t.Fatal(err)
	}
	const id = "11111111-2222-3333-4444-555555555555"
	body := `{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"2026-09-20T10:00:00.000Z","cwd":"/work","sessionId":"` + id + `"}` + "\n" +
		`{"parentUuid":"u","isSidechain":false,"message":{"model":"claude-sonnet-5","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":2}},"requestId":"req_1","type":"assistant","uuid":"a","timestamp":"2026-09-20T10:00:01.000Z","cwd":"/work","sessionId":"` + id + `"}` + "\n" +
		`{"type":"custom-title","customTitle":"Port the parser","sessionId":"` + id + `"}` + "\n"
	if err := os.WriteFile(filepath.Join(proj, id+".jsonl"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("CLAUDE_CONFIG_DIR", claude)
	sessions.Reset()
	t.Cleanup(sessions.Reset)

	out := contextOf([]gateway.Route{
		ctxRoute(1, "claude", id, 20000, 1000, 200000, 0),
		ctxRoute(2, "claude", "not-a-session", 20000, 1000, 200000, 0),
	}, 7)
	titles := map[string]string{}
	for _, s := range out.Sessions {
		titles[s.Key] = s.Title
	}
	if titles[id] != "Port the parser" || titles["not-a-session"] != "" {
		t.Fatalf("titles %v", titles)
	}
}

// masaka on Discord: a part of the context window card opens to its text.
// /api/context/text lists the part's items, and gives one item's text when
// asked for it; a request whose body is no longer kept is 404.
func TestContextTextAPI(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"id":"msg_1","type":"message","role":"assistant","model":"m1","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":90,"output_tokens":1}}`)
	}))
	defer up.Close()
	if err := provider.Save(provider.Provider{ID: "ct", Name: "CT", Anthropic: up.URL, Models: []string{"m1"}, Key: "k"}); err != nil {
		t.Fatal(err)
	}
	gw := gateway.New()
	prev := served.Swap(gw)
	t.Cleanup(func() { served.Store(prev) })
	body := `{"model":"ct/m1","max_tokens":10,"system":"You are the reviewer agent.",
		"tools":[{"name":"Read","description":"Reads a file.","input_schema":{"type":"object"}},{"name":"Grep","description":"Searches files for a long pattern, across the whole tree.","input_schema":{"type":"object"}}],
		"messages":[{"role":"user","content":"review it"}]}`
	rec := httptest.NewRecorder()
	gw.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)))
	rs := gw.Trace(context.Background(), 0, 0).Routes
	if rec.Code != 200 || len(rs) != 1 {
		t.Fatalf("%d %s", rec.Code, rec.Body)
	}
	mux := http.NewServeMux()
	contextRoutesAPI(mux)
	get := func(q string) (int, gateway.PromptText) {
		rec := httptest.NewRecorder()
		mux.ServeHTTP(rec, httptest.NewRequest("GET", "/api/context/text?"+q, nil))
		var pt gateway.PromptText
		json.Unmarshal(rec.Body.Bytes(), &pt)
		return rec.Code, pt
	}
	id := strconv.FormatInt(rs[0].ID, 10)
	code, list := get("id=" + id + "&kind=tools")
	if code != 200 || len(list.Items) != 2 || list.Items[0].Name != "Grep" || list.Items[0].Pieces != nil || list.Items[1].Pieces != nil {
		t.Fatalf("%d %+v", code, list)
	}
	code, one := get("id=" + id + "&kind=tools&item=1")
	if code != 200 || one.Items[0].Pieces != nil || len(one.Items[1].Pieces) != 1 || !strings.Contains(one.Items[1].Pieces[0].Text, "Reads a file.") {
		t.Fatalf("%d %+v", code, one)
	}
	if code, sys := get("id=" + id + "&kind=system&item=0"); code != 200 || sys.Items[0].Pieces[0].Text != "You are the reviewer agent." {
		t.Fatalf("%d %+v", code, sys)
	}
	if code, _ := get("id=1&kind=tools"); code != 404 {
		t.Fatalf("not kept: %d", code)
	}
	if code, _ := get("id=" + id + "&kind=tools&item=9"); code != 400 {
		t.Fatalf("no such item: %d", code)
	}
}
