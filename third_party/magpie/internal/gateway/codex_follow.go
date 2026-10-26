package gateway

import (
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// Codex's side calls ask for one of Codex's own models by its bare name,
// whatever the conversation is on: a subagent goes on the model its lead
// names in spawn_agent (Codex lists its own first), its memories are
// written on gpt-5.6-luna and gpt-5.6-terra, its auto-review runs on
// codex-auto-review. With the conversation on one of magpie's models
// beside Codex's ChatGPT sign-in, each of those went to that ChatGPT
// account rather than where the user had routed Codex (cybercaius on
// Discord). With nothing set for them, a subagent and the memories follow
// the session: they go on the model the conversation's own turns go on,
// when that is one of magpie's. The auto-review follows it when set to
// (settings.CodexAutoReviewSession, which codexcat writes into each of
// magpie's entries); left to Codex it stays on codex-auto-review, which a
// ChatGPT sign-in runs free (#1593).

// CodexSideSet says whether Codex's own config names the model for a side
// call ("subagent": [agents] default_subagent_model; "memories": [memories]
// extract_model or consolidation_model), true when the config can't be
// read; nil leaves every such call as Codex sent it (agent.CodexSideSet).
var CodexSideSet func(which string) bool

// codexLeadsCap bounds how many conversations codexLeads keeps.
const codexLeadsCap = 512

// codexLeads is the model each Codex conversation's own turns last went
// on, by its thread id, and the one any of them last went on, by the
// gateway key it came with.
type codexLeads struct {
	mu     sync.Mutex
	thread map[string]codexLead
	last   map[string]string
}

type codexLead struct {
	model string
	at    time.Time
}

// saw notes a turn of a Codex conversation's own (no call kind) on model.
func (l *codexLeads) saw(r *http.Request, kind, model string) {
	if kind != "" || model == "" {
		return
	}
	key := access.Caller(r.Context()).KeyID
	thread := sessionOf(r.Header)
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.thread == nil {
		l.thread, l.last = map[string]codexLead{}, map[string]string{}
	}
	l.last[key] = model
	if thread == "" {
		return
	}
	if _, ok := l.thread[thread]; !ok && len(l.thread) >= codexLeadsCap {
		oldest, at := "", time.Time{}
		for id, v := range l.thread {
			if oldest == "" || v.at.Before(at) {
				oldest, at = id, v.at
			}
		}
		delete(l.thread, oldest)
	}
	l.thread[thread] = codexLead{model: model, at: time.Now()}
}

// of is the model the conversation a side call was made for is on: its
// parent thread's (a subagent's lead, a review's conversation), else its
// own thread's, else the one this key's Codex last had a turn on.
func (l *codexLeads) of(r *http.Request, body []byte) string {
	parent := requestSessionMetadata(r.Header, body).Parent
	if parent == "" {
		parent = strings.TrimSpace(r.Header.Get("x-codex-parent-thread-id"))
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	for _, id := range []string{parent, sessionOf(r.Header)} {
		if v, ok := l.thread[id]; ok && id != "" {
			return v.model
		}
	}
	return l.last[access.Caller(r.Context()).KeyID]
}

// codexFollowTo is the model a Codex side call that asked for one of
// Codex's own models goes on instead: the session's, when it is one of
// magpie's and nothing is set for that call. "" leaves the call as it
// came: another call, a model of magpie's asked for, a model set for it, a
// conversation on one of Codex's own models, or a session model magpie
// can't serve it on now.
func (s *Server) codexFollowTo(r *http.Request, agent, kind, asked string, body []byte) string {
	if agent != "codex" && !fromCodex(r.Header) || asked == "" || strings.Contains(asked, "/") {
		return ""
	}
	which := ""
	switch {
	case usage.PurposeOf(kind) == "kind:collab_spawn":
		// a task the ChatGPT backend sealed opens only on a ChatGPT
		// account (codexSubagentPick)
		if provider.CodexSubagentModel() != "" || sealedTaskIn(body) {
			return ""
		}
		which = "subagent"
	case isMemoryKind(kind):
		which = "memories"
	default:
		return ""
	}
	to := s.codexLeads.of(r, body)
	if !strings.Contains(to, "/") || to == asked {
		return ""
	}
	if _, off := idOff(to); off {
		return ""
	}
	p, model, ok := provider.Resolve(to)
	if !ok {
		return ""
	}
	// the user's own pick in Codex's config is kept; read last, as it
	// reads the config
	if CodexSideSet == nil || CodexSideSet(which) {
		return ""
	}
	if who, held := keyHolds(r); held && !modelAllowed(who, p, model) {
		return ""
	}
	return to
}
