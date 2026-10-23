package gateway

// A turn's end, for the notification that an agent is done (itieniu on X):
// the user sets a long task going and works in another window, and wants
// to hear when it has finished.
//
// The gateway sees only requests, not turns. A turn is the requests one
// agent's session sends from the user's prompt to its last reply: each
// reply that asks the agent to run a tool is followed by another request
// once the tool has run, and the last reply asks for none. So a session's
// turn is done when a reply that called no tool has ended, nothing of the
// session is going, and nothing more came for turnQuiet after it. A
// request magpie makes itself (System One, titles answered here), one an
// agent makes on the side (Kind: titles, compaction …) and a Claude Code
// subagent's (its x-claude-code-agent-id) aren't the turn's: a subagent's
// last reply is not its parent's.

import (
	"bytes"
	"net/http"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// TurnEnd is a turn that ran at least as long as Settings' TurnDoneAlert.
type TurnEnd struct {
	Agent   string // magpie's id for it: claude, codex, opencode …
	Session string // the agent's session id, "" when it sends none
	Folder  string // the folder it says it works in, "" when it doesn't
	Took    time.Duration
}

// TurnDone is told of each such turn; set by the app, which notifies.
// Nil (magpie serve, the CLI) has turns not followed at all.
var TurnDone func(TurnEnd)

// turnQuiet is how long after a reply that called no tool nothing more of
// its session comes before its turn is done: an agent sends its next
// request at once, the reply's own follow-ups (a title, a suggestion)
// within a second or two.
var turnQuiet = 4 * time.Second

// turnAfter is how long a turn runs before its end is said; 0 says none.
var turnAfter = func() time.Duration {
	if TurnDone == nil {
		return 0
	}
	return time.Duration(settings.Load().TurnDoneAlert) * time.Second
}

// turnStale: a session with nothing for this long starts its next turn
// over, whatever its last request was (a reply read wrong as calling a
// tool, an agent closed mid-turn)
const turnStale = 30 * time.Minute

type turnState struct {
	start, last time.Time
	going       int
	folder      string
	timer       *time.Timer
}

// turns follows each conversation's turns. An agent that sends no session
// id has its concurrent turns told apart by the conversation a request
// belongs to (Conv, the first user turn's digest): without it every task
// an agent runs at once would share one clock, and a finished task's
// notification would read the longest task still going (#2001).
type turns struct {
	mu sync.Mutex
	m  map[string]*turnState
}

func sessionTurnKey(r *Route) string {
	session := r.Session
	if session == "" {
		session = r.Conv
	}
	return r.Agent + "\x00" + session
}

// begin is a request of a turn starting.
func (ts *turns) begin(r *Route, now time.Time) {
	dir := r.dirOf
	r.dirOf = nil // the body it holds isn't kept with the route
	if !r.turn {
		return
	}
	if turnAfter() <= 0 {
		return
	}
	ts.mu.Lock()
	defer ts.mu.Unlock()
	if ts.m == nil {
		ts.m = map[string]*turnState{}
	}
	for k, st := range ts.m {
		if st.going == 0 && now.Sub(st.last) > turnStale {
			if st.timer != nil {
				st.timer.Stop()
			}
			delete(ts.m, k)
		}
	}
	k := sessionTurnKey(r)
	st := ts.m[k]
	if st == nil {
		st = &turnState{start: now, last: now}
		if dir != nil {
			st.folder = dir()
		}
		ts.m[k] = st
	}
	if st.timer != nil {
		st.timer.Stop() // the agent went on: not done yet
		st.timer = nil
	}
	st.going++
}

// end is a request of a turn ending; final when its reply went whole to
// the agent and called no tool.
func (ts *turns) end(r *Route, final bool, now time.Time) {
	if !r.turn {
		return
	}
	ts.mu.Lock()
	defer ts.mu.Unlock()
	st := ts.m[sessionTurnKey(r)]
	if st == nil {
		return // begun while turns weren't followed
	}
	st.going = max(st.going-1, 0)
	st.last = now
	if st.going > 0 || !final {
		return
	}
	// read here, not in the timer: what is in force as the reply ends
	k, agent, session, after, done := sessionTurnKey(r), r.Agent, r.Session, turnAfter(), TurnDone
	var t *time.Timer
	t = time.AfterFunc(turnQuiet, func() {
		ts.mu.Lock()
		cur := ts.m[k]
		if cur == nil || cur.timer != t {
			ts.mu.Unlock()
			return // stopped, or another took its place
		}
		delete(ts.m, k)
		ts.mu.Unlock()
		if took := cur.last.Sub(cur.start); after > 0 && took >= after && done != nil {
			done(TurnEnd{Agent: agent, Session: session, Folder: cur.folder, Took: took})
		}
	})
	st.timer = t
}

// toolScan sees whether a reply going to the agent asks it to run a tool,
// in any of the wire formats: an Anthropic tool_use block or stop_reason,
// a Chat finish_reason of tool_calls (function_call before it), a
// Responses output item a client runs, a Gemini functionCall part. Only a
// JSON value is looked at ("type":"tool_use"), never a key, as a Chat
// delta may hold "tool_calls":[] with none; a string's escaped quotes
// (\"tool_use\") never match.
type toolScan struct {
	tail []byte
	saw  bool
}

var toolValues = [][]byte{
	[]byte(`"tool_use"`), []byte(`"tool_calls"`), []byte(`"function_call"`),
	[]byte(`"custom_tool_call"`), []byte(`"local_shell_call"`), []byte(`"shell_call"`),
	[]byte(`"apply_patch_call"`), []byte(`"computer_call"`), []byte(`"mcp_approval_request"`),
}

var geminiCall = []byte(`"functionCall"`)

func (s *toolScan) add(p []byte) {
	if s == nil || s.saw || len(p) == 0 {
		return
	}
	b := append(s.tail, p...)
	s.saw = toolIn(b)
	keep := min(len(b), 64) // longer than any value with its colon and spaces
	s.tail = append(s.tail[:0:0], b[len(b)-keep:]...)
}

func toolIn(b []byte) bool {
	for _, v := range toolValues {
		for i := 0; ; {
			j := bytes.Index(b[i:], v)
			if j < 0 {
				break
			}
			at := i + j
			if k := bytes.TrimRight(b[:at], " \t\r\n"); len(k) > 0 && k[len(k)-1] == ':' {
				return true
			}
			i = at + 1
		}
	}
	for i := 0; ; {
		j := bytes.Index(b[i:], geminiCall)
		if j < 0 {
			return false
		}
		at := i + j + len(geminiCall)
		if k := bytes.TrimLeft(b[at:], " \t\r\n"); len(k) > 0 && k[0] == ':' {
			return true
		}
		i = at
	}
}

// called reports whether the reply called a tool; false for a nil scan.
func (s *toolScan) called() bool { return s != nil && s.saw }

// TurnFolder is the last part of a folder, for a notification.
func TurnFolder(dir string) string {
	dir = strings.TrimRight(strings.ReplaceAll(dir, `\`, "/"), "/")
	if dir == "" {
		return ""
	}
	return filepath.Base(filepath.FromSlash(dir))
}

// agentTurnOf says whether a request is of its agent's turn, with the scan its
// reply goes through when it is: not one the agent makes on the side
// (kind) or a Claude Code subagent's.
func agentTurnOf(r *http.Request, kind string) (bool, *toolScan) {
	if sub, _ := subagentOf(r.Header); kind != "" || sub != "" {
		return false, nil
	}
	return true, &toolScan{}
}

// dirOf reads the folder body says its agent works in, when asked.
func dirOf(proto provider.Protocol, body []byte) func() string {
	return func() string {
		return factsOf(proto, body).dir
	}
}
