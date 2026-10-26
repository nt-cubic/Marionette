package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"
)

// codexFollowSetup is Codex signed in to ChatGPT, its conversation put on
// a relay's model (third/gpt-6-astra) through magpie, and Codex's config
// naming no model for its subagents or memories unless set says so.
func codexFollowSetup(t *testing.T, set bool) (*chatgptSeen, *relayOfChatGPT, *Server) {
	t.Helper()
	up := codexSubagentSetup(t)
	relay := relayOn(t, "third")
	was := CodexSideSet
	CodexSideSet = func(string) bool { return set }
	t.Cleanup(func() { CodexSideSet = was })
	s := New()
	if rec := codexSubTurn(s, "third/gpt-6-astra", "lead-1", "", leadAsk); rec.Code != 200 {
		t.Fatalf("lead: %d %s", rec.Code, rec.Body.String())
	}
	return up, relay, s
}

// codexSide is one of Codex's side calls of kind on one of its own models.
func codexSide(t *testing.T, s *Server, kind, input string) {
	t.Helper()
	if rec := codexAsks(s, "gpt-5.5", kind, input); rec.Code != 200 {
		t.Fatalf("%s: %d %s", kind, rec.Code, rec.Body.String())
	}
}

// cybercaius on Discord: with Codex on a model routed through magpie, a
// subagent its lead put on one of Codex's own models, with no model set
// for subagents, went to the ChatGPT account. It follows the session now,
// and the trace says what it asked for.
func TestCodexSubagentFollowsTheSession(t *testing.T) {
	up, relay, s := codexFollowSetup(t, false)
	codexSide(t, s, "collab_spawn", plainTask)
	if got := up.asked(); len(got) != 0 {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	reqs := relay.requests()
	if len(reqs) != 2 || modelOf([]byte(reqs[1])) != "gpt-6-astra" {
		t.Fatalf("the relay got %d requests: %v", len(reqs), reqs)
	}
	r := lastRoute(s)
	if r.Subagent == nil || *r.Subagent != (SubagentPick{Asked: "gpt-5.5", To: "third/gpt-6-astra"}) {
		t.Fatalf("trace: model %q subagent %+v", r.Model, r.Subagent)
	}
}

// Codex's memories, written on gpt-5.6-luna and -terra whatever the
// conversation is on, follow it too.
func TestCodexMemoriesFollowTheSession(t *testing.T) {
	up, relay, s := codexFollowSetup(t, false)
	codexSide(t, s, "memgen", leadAsk)
	if got := up.asked(); len(got) != 0 {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if reqs := relay.requests(); len(reqs) != 2 {
		t.Fatalf("the relay got %d requests", len(reqs))
	}
}

// A model Codex's config names for them is the user's own pick: the side
// calls go as Codex sent them.
func TestCodexSideCallsKeepTheConfigsModel(t *testing.T) {
	up, relay, s := codexFollowSetup(t, true)
	codexSide(t, s, "collab_spawn", plainTask)
	codexSide(t, s, "memgen", leadAsk)
	if got := up.asked(); len(got) != 2 || got[0] != "gpt-5.5" || got[1] != "gpt-5.5" {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if reqs := relay.requests(); len(reqs) != 1 {
		t.Fatalf("the relay got %d requests", len(reqs))
	}
}

// A conversation on Codex's own model, and a task the ChatGPT backend
// sealed (which opens on a ChatGPT account alone), stay on ChatGPT.
func TestCodexSideCallsStayOnChatGPT(t *testing.T) {
	up := codexSubagentSetup(t)
	relay := relayOn(t, "third")
	was := CodexSideSet
	CodexSideSet = func(string) bool { return false }
	t.Cleanup(func() { CodexSideSet = was })
	s := New()
	codexSide(t, s, "", leadAsk)
	codexSide(t, s, "collab_spawn", plainTask)
	if got := up.asked(); len(got) != 2 {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if rec := codexSubTurn(s, "third/gpt-6-astra", "lead-1", "", leadAsk); rec.Code != 200 {
		t.Fatalf("lead: %d", rec.Code)
	}
	codexSide(t, s, "collab_spawn", sealedTaskFernet)
	if got := up.asked(); len(got) != 3 || got[2] != "gpt-5.5" {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if reqs := relay.requests(); len(reqs) != 1 {
		t.Fatalf("the relay got %d requests", len(reqs))
	}
}

// codexAppSide is one of the Codex app's hidden side turns of a thread
// (source: thread_title, thread_description, composer_predictions), asked
// on one of Codex's own models as the app asks for them.
func codexAppSide(t *testing.T, s *Server, source, session string) {
	t.Helper()
	body := `{"model":"gpt-6-luna","stream":true,"input":[` + leadAsk + `]}`
	req := httptest.NewRequest("POST", CodexPath+"/responses", strings.NewReader(body))
	req.Header.Set("Authorization", "Bearer chatgpt-token")
	req.Header.Set("chatgpt-account-id", "acct-1")
	req.Header.Set("session_id", session)
	req.Header.Set("x-codex-turn-metadata", `{"thread_source":"`+source+`"}`)
	s.Handler().ServeHTTP(httptest.NewRecorder(), req)
}

// cybercaius on Discord: with the conversation put on one of magpie's
// models in the Codex app, and nothing in Codex's config, the app's thread
// titles, descriptions and composer predictions still went to ChatGPT on
// its own models. They follow the session now, as the subagents do.
func TestCodexAppSideTurnsFollowTheSession(t *testing.T) {
	up, relay, s := codexFollowSetup(t, false)
	was := CodexSideModel
	CodexSideModel = func() string { return "" }
	t.Cleanup(func() { CodexSideModel = was })
	for _, src := range []string{"composer_predictions", "thread_title", "thread_description"} {
		codexAppSide(t, s, src, "lead-1")
		if got := up.asked(); len(got) != 0 {
			t.Fatalf("%s: ChatGPT was asked for %v", src, got)
		}
		reqs := relay.requests()
		if m := modelOf([]byte(reqs[len(reqs)-1])); len(reqs) < 2 || m != "gpt-6-astra" {
			t.Fatalf("%s: the relay got %d requests, the last on %q", src, len(reqs), m)
		}
	}
	if reqs := relay.requests(); len(reqs) != 4 {
		t.Fatalf("the relay got %d requests", len(reqs))
	}
}

// A conversation on Codex's own model keeps its side turns on ChatGPT,
// even with one of magpie's models in Codex's config: they follow the
// session, not the config.
func TestCodexAppSideTurnsStayWithAnOwnModelSession(t *testing.T) {
	up := codexSubagentSetup(t)
	relay := relayOn(t, "third")
	was := CodexSideModel
	CodexSideModel = func() string { return "third/gpt-6-astra" }
	t.Cleanup(func() { CodexSideModel = was })
	s := New()
	codexSide(t, s, "", leadAsk)
	codexAppSide(t, s, "thread_title", "worker-1")
	if got := up.asked(); len(got) != 2 || got[1] != "gpt-6-luna" {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if reqs := relay.requests(); len(reqs) != 0 {
		t.Fatalf("the relay got %d requests", len(reqs))
	}
}

// Codex consolidates its memories as it starts, before any turn of its
// has come through magpie: with one of magpie's models in its config,
// they go on that model, not to ChatGPT.
func TestCodexMemoriesBeforeAnyTurnFollowTheConfig(t *testing.T) {
	up := codexSubagentSetup(t)
	relay := relayOn(t, "third")
	wasSet, wasModel := CodexSideSet, CodexSideModel
	CodexSideSet = func(string) bool { return false }
	CodexSideModel = func() string { return "third/gpt-6-astra" }
	t.Cleanup(func() { CodexSideSet, CodexSideModel = wasSet, wasModel })
	s := New()
	codexSide(t, s, "memgen", leadAsk)
	if got := up.asked(); len(got) != 0 {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if reqs := relay.requests(); len(reqs) != 1 || modelOf([]byte(reqs[0])) != "gpt-6-astra" {
		t.Fatalf("the relay got %v", reqs)
	}
}
