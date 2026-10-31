package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

const workerAsk = `{"type":"message","role":"user","content":[{"type":"input_text","text":"go on"}]}`

// codexThreadAsks is a request of Codex's as a recent Codex sends it: the
// session_id shared by the root thread and every subagent under it, the
// thread's own id in its turn metadata; a subagent's when parent is set.
func codexThreadAsks(s *Server, model, thread, parent string) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	body := `{"model":"` + model + `","stream":true,"input":[` + workerAsk + `]}`
	req := httptest.NewRequest("POST", CodexPath+"/responses", strings.NewReader(body))
	req.Header.Set("Authorization", "Bearer chatgpt-token")
	req.Header.Set("chatgpt-account-id", "acct-1")
	req.Header.Set("User-Agent", "codex_cli_rs/0.162.1 (Mac OS 26.2.0; arm64)")
	req.Header.Set("session_id", "root-1")
	meta := `{"session_id":"root-1","thread_id":"` + thread + `"`
	if parent != "" {
		req.Header.Set("x-openai-subagent", "collab_spawn")
		req.Header.Set("x-codex-parent-thread-id", parent)
		meta += `,"parent_thread_id":"` + parent + `"`
	}
	req.Header.Set("x-codex-turn-metadata", meta+"}")
	s.Handler().ServeHTTP(rec, req)
	return rec
}

// HoGee_xxl on X: Codex's resume_agent builds the resumed subagent's
// config from its lead's turn (codex-rs core/src/agent/child_config.rs,
// build_agent_resume_config), so a subagent spawned on gpt-6-astra came
// back on the lead's gpt-5.5. It goes back on gpt-6-astra; another
// subagent, and the lead, go on as asked. Remembered across a restart.
func TestCodexResumedSubagentKeepsItsSpawnModel(t *testing.T) {
	up := codexSubagentSetup(t)
	s := New()
	steps := []struct{ model, thread, parent, want string }{
		{"gpt-5.5", "root-1", "", "gpt-5.5"},                 // the lead
		{"gpt-6-astra", "worker-1", "root-1", "gpt-6-astra"}, // spawned on astra
		{"gpt-5.5", "worker-2", "root-1", "gpt-5.5"},         // spawned on the lead's
		{"gpt-5.5", "worker-1", "root-1", "gpt-6-astra"},     // resumed on the lead's
		{"gpt-5.5", "root-1", "", "gpt-5.5"},                 // the lead's own, untouched
	}
	for i, st := range steps {
		if rec := codexThreadAsks(s, st.model, st.thread, st.parent); rec.Code != 200 {
			t.Fatalf("step %d: %d %s", i, rec.Code, rec.Body.String())
		}
		if got := up.asked(); got[len(got)-1] != st.want {
			t.Fatalf("step %d (%s asking %s) went on %s, want %s", i, st.thread, st.model, got[len(got)-1], st.want)
		}
	}
	// a magpie started afresh still knows worker-1
	if rec := codexThreadAsks(New(), "gpt-5.5", "worker-1", "root-1"); rec.Code != 200 {
		t.Fatalf("after a restart: %d %s", rec.Code, rec.Body.String())
	}
	if got := up.asked(); got[len(got)-1] != "gpt-6-astra" {
		t.Fatalf("after a restart worker-1 went on %s", got[len(got)-1])
	}
}

// A model set for every subagent still holds a resumed one to it.
func TestCodexResumedSubagentHeldToTheModelSet(t *testing.T) {
	up := codexSubagentSetup(t)
	s := New()
	codexThreadAsks(s, "gpt-5.5", "worker-1", "root-1")
	if err := provider.SetCodexSubagentModel("codex/gpt-6-astra"); err != nil {
		t.Fatal(err)
	}
	if rec := codexThreadAsks(s, "gpt-5.5", "worker-1", "root-1"); rec.Code != 200 {
		t.Fatalf("%d %s", rec.Code, rec.Body.String())
	}
	if got := up.asked(); got[len(got)-1] != "gpt-6-astra" {
		t.Fatalf("held subagent went on %s", got[len(got)-1])
	}
}

// A Codex from before session_id was shared names a subagent's thread by
// its session_id; a recent one whose metadata lacks thread_id shares the
// lead's session_id, and is left as asked rather than tied to the lead.
func TestCodexSubagentThreadOf(t *testing.T) {
	h := httptest.NewRequest("POST", "/", nil).Header
	h.Set("session_id", "worker-9")
	h.Set("x-codex-parent-thread-id", "lead-9")
	if got := codexSubagentThread(h, sessionMetadata{}); got != "worker-9" {
		t.Fatalf("older Codex: %q", got)
	}
	h.Set("session_id", "lead-9")
	if got := codexSubagentThread(h, sessionMetadata{}); got != "" {
		t.Fatalf("shared session id taken as the thread: %q", got)
	}
	if got := codexSubagentThread(h, sessionMetadata{Thread: "worker-10"}); got != "worker-10" {
		t.Fatalf("thread_id: %q", got)
	}
}
