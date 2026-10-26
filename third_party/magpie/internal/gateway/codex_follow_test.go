package gateway

import "testing"

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
