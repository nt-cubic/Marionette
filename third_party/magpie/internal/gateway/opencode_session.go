package gateway

import (
	"log"
	"net/http"
	"slices"
	"strings"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
)

// openCodeSeed names the conversation a request to OpenCode's gateway is a
// turn of, for its x-opencode-session (provider.OpenCodeClient): the
// client's own x-opencode-session when it sent one, else the session the
// client names (X-Magpie-Session, Codex's session_id, Claude Code's
// session, Pi's affinity: sessionOf), else the body's prompt_cache_key
// (Codex's thread), else the conversation's first user message
// (conversationID). OpenCode itself sends one session id for every turn of
// a session (packages/core/src/session/affinity.ts), and Zen and Go route
// a session to the backend that has its prompt cached: every turn of one
// conversation names the same, and two conversations never one. Without
// the cache key, two of Codex's conversations in one folder, which open on
// the same environment message, were one session.
func openCodeSeed(in http.Header, body []byte) string {
	if v := strings.TrimSpace(in.Get("x-opencode-session")); v != "" {
		return v
	}
	if v := sessionOf(in); v != "" {
		return v
	}
	if k := strings.TrimSpace(gjson.GetBytes(body, "prompt_cache_key").String()); k != "" {
		return "prompt_cache_key\x00" + k
	}
	return conversationID(in, body)
}

// logOpenCodeRefusal writes to magpie's log what went to OpenCode's
// gateway with a request it refused: where, the names of the headers sent
// (never a value), and whether x-opencode-session was among them, by its
// first 8 characters. OpenCode answers a request without one 400
// "Request is missing x-opencode-session and cannot be routed
// efficiently" (luci on Discord), and this tells that apart from a
// session the user's own header pins every conversation to.
func logOpenCodeRefusal(p provider.Provider, req *http.Request, status int) {
	var names []string
	for k := range req.Header {
		names = append(names, strings.ToLower(k))
	}
	slices.Sort(names)
	session := "missing"
	if v := req.Header.Get("x-opencode-session"); v != "" {
		session = v[:min(len(v), 8)] + "…"
		for k, v := range p.Headers {
			// ${sessionId} in it is each conversation's own
			if strings.EqualFold(k, "x-opencode-session") && !strings.Contains(v, provider.SessionPlaceholder) && !strings.Contains(v, provider.ThreadPlaceholder) {
				session += " (the provider's own header: one session for every conversation)"
			}
		}
	}
	log.Printf("opencode: %s answered %d to %s %s%s; x-opencode-session %s; headers sent: %s",
		p.ID, status, req.Method, req.URL.Host, req.URL.Path, session, strings.Join(names, ", "))
}
