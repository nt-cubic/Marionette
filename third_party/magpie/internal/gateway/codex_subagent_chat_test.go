package gateway

import (
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #1633 (laacmilan): Codex's lead on token/gpt-6.1-sol, a provider on Chat
// Completions, its subagents wanted on token too. The lead's spawn_agent
// task reaches the subagent as text (magpie served the lead), so a
// subagent the lead put on one of Codex's own models goes on the token
// model set for Codex's subagents, and ChatGPT isn't asked. The lead's
// own turn stays on the model it asked for.
func TestCodexSubagentPutOnAChatProviderUnderAChatLead(t *testing.T) {
	up := codexSubagentSetup(t)
	token := chatRecordedOn(t, "token", "gpt-6.1-sol")
	if err := provider.SetCodexSubagentModel("token/gpt-6.1-sol"); err != nil {
		t.Fatal(err)
	}
	s := New()
	rec := codexAsks(s, "gpt-6-astra", "collab_spawn", plainTask)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "ok") {
		t.Fatalf("subagent: %d %s", rec.Code, rec.Body.String())
	}
	if got := up.asked(); len(got) != 0 {
		t.Fatalf("ChatGPT was asked for %v", got)
	}
	if got := token.requests(); len(got) != 1 || !strings.Contains(got[0], `"gpt-6.1-sol"`) || !strings.Contains(got[0], "read the README") {
		t.Fatalf("token was asked %v", got)
	}
	r := lastRoute(s)
	if r.Subagent == nil || *r.Subagent != (SubagentPick{Asked: "gpt-6-astra", To: "token/gpt-6.1-sol"}) || r.Provider != "token" {
		t.Fatalf("trace: provider %q subagent %+v", r.Provider, r.Subagent)
	}
	// the lead on its own model, magpie's token one
	if rec := codexAsks(s, "token/gpt-6.1-sol", "", leadAsk); rec.Code != 200 {
		t.Fatalf("lead: %d %s", rec.Code, rec.Body.String())
	}
	if r := lastRoute(s); r.Subagent != nil {
		t.Fatalf("lead traced %+v", r.Subagent)
	}
}

// #1613's other side: the lead on one of Codex's own models, answered by
// the ChatGPT backend, which sealed the subagent's task. token can't open
// it, so the subagent stays on the model its lead asked for — answered by
// ChatGPT as with no model set, not turned away — and the trace says why.
// A subagent's request holding only a sealed message, not a task, still
// goes on token: that message is left out with a note (#1613).
func TestCodexSubagentSealedTaskStaysOffAChatProvider(t *testing.T) {
	up := codexSubagentSetup(t)
	token := chatRecordedOn(t, "token", "gpt-6.1-sol")
	if err := provider.SetCodexSubagentModel("token/gpt-6.1-sol"); err != nil {
		t.Fatal(err)
	}
	s := New()
	rec := codexAsks(s, "gpt-5.5", "collab_spawn", sealedTaskFernet)
	if rec.Code != 200 || !strings.Contains(rec.Body.String(), "on it") {
		t.Fatalf("subagent: %d %s", rec.Code, rec.Body.String())
	}
	if got := up.asked(); len(got) != 1 || got[0] != "gpt-5.5" {
		t.Fatalf("ChatGPT was asked for %v, not the model asked", got)
	}
	if n := len(token.requests()); n != 0 {
		t.Fatalf("token was asked %d times", n)
	}
	if r := lastRoute(s); r.Subagent == nil || r.Subagent.To != "token/gpt-6.1-sol" || r.Subagent.Kept != sealedKept {
		t.Fatalf("trace: %+v", r.Subagent)
	}
	rec = codexAsks(s, "gpt-5.5", "collab_spawn", plainTask+","+sealedWorkerMessage)
	if rec.Code != 200 {
		t.Fatalf("subagent with a sealed message: %d %s", rec.Code, rec.Body.String())
	}
	if got := token.requests(); len(got) != 1 || strings.Contains(got[0], "gAAAAATest_worker_message") {
		t.Fatalf("token was asked %v", got)
	}
	if r := lastRoute(s); !r.Subagent.Moved() {
		t.Fatalf("trace: %+v", r.Subagent)
	}
}
