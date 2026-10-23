package sessions

import (
	"bytes"
	"encoding/json"
	"strings"
	"time"

	"github.com/tidwall/gjson"
)

// Codex writes a rollout file per session (a session picked up again later
// may go on in another, named with the same thread id): a session_meta line,
// a turn_context per turn naming the model, and token_count events with the
// running total and the last call's usage. Its input counts include the
// cached tokens.

type cxLine struct {
	Type    string `json:"type"`
	Payload struct {
		cxCompaction
		cxHistorySnapshot
		Type           string          `json:"type"`
		SessionID      string          `json:"session_id"`
		ThreadID       string          `json:"thread_id"`
		ParentThreadID string          `json:"parent_thread_id"`
		Source         json.RawMessage `json:"source"`
		TurnID         string          `json:"turn_id"`
		ResponseID     string          `json:"response_id"`
		Usage          *cxUsage        `json:"usage"`
		ThreadUsage    *cxUsage        `json:"thread_token_usage"`
		ID             string          `json:"id"`
		Cwd            string          `json:"cwd"`
		Model          string          `json:"model"`
		// session_meta's
		ModelProvider string `json:"model_provider"`
		Role          string `json:"role"`
		Message       string `json:"message"`
		AgentNickname string `json:"agent_nickname"`
		CallID        string `json:"call_id"`
		Name          string `json:"name"`
		Arguments     string `json:"arguments"`
		// a legacy rollout's sub_agent_activity event
		EventID       string `json:"event_id"`
		AgentThreadID string `json:"agent_thread_id"`
		Kind          string `json:"kind"`
		Item          struct {
			ID            string `json:"id"`
			Type          string `json:"type"`
			Kind          string `json:"kind"`
			AgentThreadID string `json:"agent_thread_id"`
		} `json:"item"`
		Content []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
		Settings struct {
			Model string `json:"model"`
			Cwd   string `json:"cwd"`
		} `json:"thread_settings"`
		Info *struct {
			Total *cxUsage `json:"total_token_usage"`
			Last  *cxUsage `json:"last_token_usage"`
		} `json:"info"`
	} `json:"payload"`
}

type cxUsage struct {
	Input      int   `json:"input_tokens"`
	Cached     int   `json:"cached_input_tokens"`
	CacheWrite int   `json:"cache_write_input_tokens"`
	Output     int   `json:"output_tokens"`
	Reasoning  int   `json:"reasoning_output_tokens"`
	Known      uint8 `json:"known,omitempty"`
}

// raw is the usage in Codex's own terms, input with the cache in it.
func (u cxUsage) raw() Tokens {
	return Tokens{Input: u.Input, Output: u.Output, CacheRead: u.Cached, CacheWrite: u.CacheWrite}
}

// spent is raw usage with the cache taken out of the input: Codex's
// input_tokens is the whole prompt, what was read from the cache and what
// was written to it (cache_write_input_tokens, which a newer Codex keeps)
// among it, so neither is counted twice (#589).
func spent(t Tokens) Tokens {
	t.Input -= t.CacheRead + t.CacheWrite
	if t.Input < 0 {
		t.Input = 0
	}
	return t
}

var (
	cxMeta             = []byte(`"type":"session_meta"`)
	cxTurn             = []byte(`"type":"turn_context"`)
	cxRecord           = []byte(`"type":"token_usage_record"`)
	cxCompacted        = []byte(`"type":"compacted"`)
	cxCount            = []byte(`"type":"token_count"`)
	cxSettings         = []byte(`"type":"thread_settings_applied"`)
	cxUserMsg          = []byte(`"type":"user_message"`)
	cxUserRole         = []byte(`"role":"user"`)
	cxRole             = []byte(`"role":"`)
	cxType             = []byte(`"type":"`)
	cxPayload          = []byte(`"payload":{"type":"`)
	cxText             = []byte(`,"text":"`)
	cxSpawnAgent       = []byte(`"name":"spawn_agent"`)
	cxSubAgentActivity = []byte(`"type":"SubAgentActivity"`)
	cxSubAgentEvent    = []byte(`"type":"sub_agent_activity"`)
	cxStartedKind      = []byte(`"kind":"started"`)
)

func codexLine(s *state, b []byte, main bool) {
	if codexHead(s, b, main) {
		codexBody(s, b, main)
	}
}

// codexHead notes the time of a line, from its start, and says whether the
// rest of it is worth reading: from its type and its payload's, which a
// rollout writes first, so irrelevant tool output is never searched.
// Compactions carry usage metadata. A line laid out otherwise is read.
func codexHead(s *state, b []byte, main bool) bool {
	at := tsAt(b[:min(len(b), 256)], false)
	if at.IsZero() {
		at = tsAt(b, false)
	}
	s.saw(at, main)
	h := b[:min(len(b), 1024)]
	top := typeAfter(h, cxType)
	if s.Codex != nil && top == "response_item" {
		payload := typeAfter(h, cxPayload)
		if payload == "message" && bytes.Contains(h, cxUserRole) {
			s.Codex.Pending = 0
		}
	}
	if s.Codex != nil && top == "event_msg" && typeAfter(h, cxPayload) == "user_message" {
		s.Codex.Pending = 0
	}
	switch top {
	case "":
		return true
	case "session_meta":
		return s.ID == ""
	case "turn_context", "token_usage_record", "compacted":
		return true
	case "event_msg", "response_item":
	default:
		return false
	}
	payload := typeAfter(h, cxPayload)
	switch {
	case payload == "":
		return true
	case top == "event_msg":
		return payload == "token_count" || payload == "task_started" || payload == "thread_settings_applied" || payload == "user_message" && s.Title == "" ||
			(payload == "sub_agent_activity" || bytes.Contains(h, cxSubAgentActivity)) && bytes.Contains(h, cxStartedKind)
	case cxCalls[payload]:
		// a tool call, named in its head
		name := typeAfter(h, cxName)
		if payload != "function_call" && payload != "custom_tool_call" || name == "" {
			name = payload
		}
		s.tool(at, name, "")
		return top == "response_item" && payload == "function_call" && name == "spawn_agent"
	case payload == "message" && typeAfter(h, cxRole) == "assistant":
		if main {
			s.day(dateOf(at)).Replies++
		}
		return false
	default:
		return payload == "message" && (main || s.Title == "") && (!bytes.Contains(h, cxRole) || bytes.Contains(h, cxUserRole))
	}
}

// cxCalls are the kinds of response item that call a tool.
var cxCalls = map[string]bool{"function_call": true, "custom_tool_call": true, "local_shell_call": true, "web_search_call": true, "tool_search_call": true, "image_generation_call": true}

var cxName = []byte(`"name":"`)

const (
	codexRoleMain     = "main"
	codexRoleSubagent = "subagent"
	codexRoleUnknown  = "unknown"
)

func codexIdentityFromMeta(id string, source json.RawMessage, topParent string) (CodexIdentity, bool) {
	identity := CodexIdentity{ID: id, Role: codexRoleUnknown}
	role, sourceParent := codexSource(source)
	if sourceParent != "" && topParent != "" && sourceParent != topParent || role == codexRoleMain && topParent != "" {
		return identity, true
	}
	if sourceParent == "" {
		sourceParent = topParent
	}
	if role == codexRoleUnknown && sourceParent != "" {
		role = codexRoleSubagent
	}
	identity.Role, identity.ParentSession = role, sourceParent
	return identity, false
}

func codexSource(source json.RawMessage) (string, string) {
	var name string
	if json.Unmarshal(source, &name) == nil {
		switch name {
		case "cli", "vscode", "exec", "mcp":
			return codexRoleMain, ""
		default:
			return codexRoleUnknown, ""
		}
	}
	var variants map[string]json.RawMessage
	if json.Unmarshal(source, &variants) != nil || len(variants) != 1 {
		return codexRoleUnknown, ""
	}
	if raw, ok := variants["custom"]; ok {
		var custom string
		if json.Unmarshal(raw, &custom) == nil && custom != "" {
			return codexRoleMain, ""
		}
		return codexRoleUnknown, ""
	}
	raw, ok := variants["subagent"]
	if !ok {
		return codexRoleUnknown, ""
	}
	var kind map[string]json.RawMessage
	if json.Unmarshal(raw, &kind) != nil || len(kind) != 1 {
		return codexRoleSubagent, ""
	}
	raw, ok = kind["thread_spawn"]
	if !ok {
		return codexRoleSubagent, ""
	}
	var thread struct {
		Parent string `json:"parent_thread_id"`
	}
	if json.Unmarshal(raw, &thread) != nil {
		return codexRoleSubagent, ""
	}
	return codexRoleSubagent, thread.Parent
}

func mergeCodexIdentity(old, next CodexIdentity) (CodexIdentity, bool) {
	if old.ID != next.ID || old.Role != codexRoleUnknown && next.Role != codexRoleUnknown && old.Role != next.Role ||
		old.ParentSession != "" && next.ParentSession != "" && old.ParentSession != next.ParentSession {
		return CodexIdentity{ID: old.ID, Role: codexRoleUnknown}, true
	}
	if old.Role == codexRoleUnknown {
		old.Role = next.Role
	}
	if old.ParentSession == "" {
		old.ParentSession = next.ParentSession
	}
	return old, false
}

func noteCodexIdentity(s *state, identity CodexIdentity, conflict bool) {
	if s.CodexIdentity == nil {
		copy := identity
		s.CodexIdentity, s.CodexIdentityConflict = &copy, conflict
		return
	}
	if s.CodexIdentityConflict {
		return
	}
	old := *s.CodexIdentity
	merged, conflictFound := mergeCodexIdentity(old, identity)
	if conflict || conflictFound {
		s.CodexIdentity = &CodexIdentity{ID: old.ID, Role: codexRoleUnknown}
		s.CodexIdentityConflict = true
		return
	}
	s.CodexIdentity = &merged
}

// CodexIdentities returns an independent snapshot of the native thread identities.
func CodexIdentities() map[string]CodexIdentity {
	storeMu.RLock()
	defer storeMu.RUnlock()
	mu.Lock()
	defer mu.Unlock()
	loadCache()
	defer useDBs()()
	want := append(codexFiles(), wslFiles("codex")...)
	refresh(want, nil)
	return codexIdentitySnapshot(want)
}

func setSessionCodexIdentity(s *Session, identities map[string]CodexIdentity) {
	if s.Agent != "codex" {
		return
	}
	if identity, ok := identities[s.ID]; ok {
		s.Role, s.ParentSession, s.RootSession = identity.Role, identity.ParentSession, identity.RootSession
	}
}

// codexTaskTitle names a child thread by the parent whose spawn_agent call
// started it: a forked child's file repeats its parent's history, spawns of
// its siblings included, and those claims never stand for the real parent's.
type codexTaskTitle struct {
	child  string
	parent string
}

func codexSubagentTaskTitles(files []file) map[codexTaskTitle]string {
	type parentCalls struct {
		names       map[string]string
		children    map[string]string
		badNames    map[string]bool
		badChildren map[string]bool
	}
	parents := map[string]*parentCalls{}
	for _, f := range files {
		if f.agent != "codex" || cache[f.path] == nil {
			continue
		}
		calls := parents[f.key]
		if calls == nil {
			calls = &parentCalls{names: map[string]string{}, children: map[string]string{}, badNames: map[string]bool{}, badChildren: map[string]bool{}}
			parents[f.key] = calls
		}
		st := cache[f.path]
		for id := range st.CodexSpawnConflicts {
			calls.badNames[id] = true
		}
		for id := range st.CodexAgentConflicts {
			calls.badChildren[id] = true
		}
		for id, name := range st.CodexSpawnNames {
			if old := calls.names[id]; old != "" && old != name {
				calls.badNames[id] = true
			}
			calls.names[id] = name
		}
		for id, child := range st.CodexAgentIDs {
			if old := calls.children[id]; old != "" && old != child {
				calls.badChildren[id] = true
			}
			calls.children[id] = child
		}
	}
	out, conflicts := map[codexTaskTitle]string{}, map[codexTaskTitle]bool{}
	for parentKey, calls := range parents {
		parent := strings.TrimPrefix(parentKey, "codex:")
		for id, name := range calls.names {
			child := calls.children[id]
			if calls.badNames[id] || calls.badChildren[id] || name == "" || child == "" {
				continue
			}
			link := codexTaskTitle{child: child, parent: parent}
			if old, ok := out[link]; ok && old != name {
				conflicts[link] = true
			} else {
				out[link] = name
			}
		}
	}
	for link := range conflicts {
		delete(out, link)
	}
	return out
}

func codexTaskParentFiles(children, all []file) []file {
	parents := map[string]bool{}
	for _, f := range children {
		if f.agent != "codex" {
			continue
		}
		st := cache[f.path]
		if st != nil && !st.CodexIdentityConflict && st.CodexIdentity != nil && st.CodexIdentity.Role == codexRoleSubagent && st.CodexIdentity.ParentSession != "" {
			parents[st.CodexIdentity.ParentSession] = true
		}
	}
	var out []file
	for _, f := range all {
		if f.agent == "codex" && parents[strings.TrimPrefix(f.key, "codex:")] {
			out = append(out, f)
		}
	}
	return out
}

// codexAgentStarted notes that the spawn call id started the child thread.
func (s *state) codexAgentStarted(id, thread string) {
	if id == "" || thread == "" {
		return
	}
	if s.CodexAgentIDs == nil {
		s.CodexAgentIDs = map[string]string{}
	}
	if old, ok := s.CodexAgentIDs[id]; ok && old != thread {
		if s.CodexAgentConflicts == nil {
			s.CodexAgentConflicts = map[string]bool{}
		}
		s.CodexAgentConflicts[id] = true
	} else {
		s.CodexAgentIDs[id] = thread
	}
}

func setSessionCodexTaskTitle(s *Session, titles map[codexTaskTitle]string) {
	if s.Agent != "codex" || s.Title != "" || s.Role != codexRoleSubagent || s.ParentSession == "" {
		return
	}
	if name, ok := titles[codexTaskTitle{child: s.ID, parent: s.ParentSession}]; ok {
		s.Title = name
	}
}

func codexIdentitySnapshot(files []file) map[string]CodexIdentity {
	identities := map[string]CodexIdentity{}
	conflicts := map[string]bool{}
	for _, f := range files {
		if f.agent != "codex" {
			continue
		}
		id := strings.TrimPrefix(f.key, "codex:")
		identity := CodexIdentity{ID: id, Role: codexRoleUnknown}
		conflict := false
		if s := cache[f.path]; s != nil && s.CodexIdentity != nil {
			identity = *s.CodexIdentity
			conflict = s.CodexIdentityConflict
			if identity.ID == "" {
				identity.ID = id
			}
			if identity.Role == "" {
				identity.Role = codexRoleUnknown
			}
		}
		if identity.ID == "" {
			continue
		}
		if conflict {
			conflicts[identity.ID] = true
		}
		old, ok := identities[identity.ID]
		if !ok {
			identities[identity.ID] = identity
			continue
		}
		merged, conflict := mergeCodexIdentity(old, identity)
		if conflict {
			conflicts[identity.ID] = true
		} else {
			identities[identity.ID] = merged
		}
	}
	for id := range conflicts {
		identities[id] = CodexIdentity{ID: id, Role: codexRoleUnknown}
	}
	for id, identity := range identities {
		if identity.Role == codexRoleMain {
			identity.RootSession = id
			identities[id] = identity
		}
	}
	for id, identity := range identities {
		if identity.Role != codexRoleSubagent {
			continue
		}
		identity.RootSession = codexRoot(identity, identities)
		identities[id] = identity
	}
	return identities
}

func codexRoot(identity CodexIdentity, identities map[string]CodexIdentity) string {
	seen := map[string]bool{identity.ID: true}
	parent := identity.ParentSession
	for parent != "" {
		if seen[parent] {
			return ""
		}
		seen[parent] = true
		ancestor, ok := identities[parent]
		if !ok {
			return ""
		}
		switch ancestor.Role {
		case codexRoleMain:
			return ancestor.ID
		case codexRoleSubagent:
			parent = ancestor.ParentSession
		default:
			return ""
		}
	}
	return ""
}

// typeAfter is the word in quotes after the first key in b, or "".
func typeAfter(b, key []byte) string {
	i := bytes.Index(b, key)
	if i < 0 {
		return ""
	}
	rest := b[i+len(key):]
	j := bytes.IndexByte(rest, '"')
	if j < 0 {
		return ""
	}
	return string(rest[:j])
}

// Read compaction identity/usage without decoding its potentially multi-MB
// replacement history. The parser needs none of that conversation content.
func cxCompactionIn(b []byte) cxCompaction {
	fields := gjson.GetManyBytes(b, "payload.compaction_response_id", "payload.latest_token_usage_record")
	p := cxCompaction{ResponseID: fields[0].String()}
	if fields[1].Exists() {
		_ = json.Unmarshal([]byte(fields[1].Raw), &p.Record)
	}
	return p
}

// codexBody reads a line codexHead let through.
func codexBody(s *state, b []byte, main bool) {
	at := tsAt(b[:min(len(b), 256)], false)
	if at.IsZero() {
		at = tsAt(b, false)
	}
	if typeAfter(b[:min(len(b), 1024)], cxType) == "compacted" {
		codexApply(s, codexState(s).compacted(at, s.Model, cxCompactionIn(b)))
		return
	}
	// the two kinds of line most read, each told from its head
	if cxFastCount(s, at, b) || cxFastPrompt(s, at, b, main) {
		return
	}
	want := s.ID == "" && bytes.Contains(b, cxMeta) ||
		bytes.Contains(b, cxTurn) || bytes.Contains(b, cxCount) || bytes.Contains(b, cxRecord) || bytes.Contains(b, cxCompacted) || bytes.Contains(b, cxStarted) || bytes.Contains(b, cxSettings) ||
		s.Title == "" && bytes.Contains(b, cxUserMsg) || (main || s.Title == "") && bytes.Contains(b, cxUserRole) ||
		bytes.Contains(b, cxSpawnAgent) || (bytes.Contains(b, cxSubAgentActivity) || bytes.Contains(b, cxSubAgentEvent)) && bytes.Contains(b, cxStartedKind)
	if !want {
		return
	}
	var l cxLine
	if json.Unmarshal(b, &l) != nil {
		return
	}
	p := &l.Payload
	switch {
	case l.Type == "compacted":
		codexApply(s, codexState(s).compacted(at, s.Model, p.cxCompaction))
	case l.Type == "token_usage_record":
		if p.Usage != nil {
			codexApply(s, codexState(s).record(codexContribution{ResponseID: p.ResponseID, Session: p.SessionID, ThreadID: p.ThreadID, Turn: p.TurnID, Model: s.Model, At: at, Usage: *p.Usage, Total: p.ThreadUsage}))
		}
	case l.Type == "session_meta":
		identity, conflict := codexIdentityFromMeta(p.ID, p.Source, p.ParentThreadID)
		if identity.ID != "" {
			noteCodexIdentity(s, identity, conflict)
		}
		session := p.SessionID
		if session == "" {
			session = p.ID
		}
		codexState(s).context(session, "", "")
		codexState(s).beginSnapshot(at, p.ID, p.cxHistorySnapshot)
		if s.ID == "" {
			s.ID = p.ID
			if s.Provider = p.ModelProvider; s.Provider == "" {
				s.Provider = "openai"
			}
		}
		if s.Cwd == "" {
			s.Cwd = p.Cwd
		}
		if p.AgentNickname != "" {
			s.CodexNickname = p.AgentNickname
		}
	case l.Type == "response_item" && p.Type == "function_call" && p.Name == "spawn_agent":
		if p.CallID != "" {
			if name := title(gjson.Get(p.Arguments, "task_name").String()); name != "" {
				if s.CodexSpawnNames == nil {
					s.CodexSpawnNames = map[string]string{}
				}
				if old, ok := s.CodexSpawnNames[p.CallID]; ok && old != name {
					if s.CodexSpawnConflicts == nil {
						s.CodexSpawnConflicts = map[string]bool{}
					}
					s.CodexSpawnConflicts[p.CallID] = true
				} else {
					s.CodexSpawnNames[p.CallID] = name
				}
			}
		}
	case l.Type == "event_msg" && p.Type == "item_completed" && p.Item.Type == "SubAgentActivity" && p.Item.Kind == "started":
		s.codexAgentStarted(p.Item.ID, p.Item.AgentThreadID)
	case l.Type == "event_msg" && p.Type == "sub_agent_activity" && p.Kind == "started":
		// a legacy rollout (Codex's default history before paginated) keeps
		// the started activity as this event, its id the spawn call's
		s.codexAgentStarted(p.EventID, p.AgentThreadID)
	case l.Type == "turn_context":
		codexState(s).context("", p.TurnID, p.Model)
		if p.Model != "" {
			s.Model = p.Model
		}
		if s.Cwd == "" {
			s.Cwd = p.Cwd
		}
	case l.Type == "event_msg" && p.Type == "task_started":
		codexState(s).context("", p.TurnID, "")
		codexState(s).Pending = 0
	case l.Type == "event_msg" && p.Type == "thread_settings_applied":
		if p.Settings.Model != "" {
			s.Model = p.Settings.Model
			codexState(s).context("", "", p.Settings.Model)
		}
	case l.Type == "event_msg" && p.Type == "user_message":
		if s.Title == "" {
			s.Title = cxPrompt(p.Message)
		}
		if s.Title == "" && s.First == "" {
			s.First = untagged(p.Message)
		}
	case l.Type == "response_item" && p.Type == "message" && p.Role == "user":
		typed := false
		for _, c := range p.Content {
			if c.Type == "input_text" || c.Type == "text" {
				t := cxPrompt(c.Text)
				typed = typed || t != ""
				if s.Title == "" {
					s.Title = t
				}
			}
		}
		if typed && main {
			s.day(dateOf(at)).Prompts++
		}
	case l.Type == "response_item" && p.Type == "message" && p.Role == "assistant":
		if main {
			s.day(dateOf(at)).Replies++
		}
	case l.Type == "event_msg" && p.Type == "token_count":
		if p.Info != nil {
			cxCounted(s, at, p.Info.Total, p.Info.Last)
		}
	}
}

// cxFastPrompt reads a prompt's texts out of a user message without decoding
// the images it may carry: a content part's type and text are written in
// that order, and the key can't be in a string, where its quotes would be
// escaped. A message laid out otherwise is left to be read whole.
func cxFastPrompt(s *state, at time.Time, b []byte, main bool) bool {
	h := b[:min(len(b), 1024)]
	if typeAfter(h, cxType) != "response_item" || typeAfter(h, cxPayload) != "message" || typeAfter(h, cxRole) != "user" {
		return false
	}
	found, typed := false, false
	for rest := b; ; {
		i := bytes.Index(rest, cxType)
		if i < 0 {
			if typed && main {
				s.day(dateOf(at)).Prompts++
			}
			// no text at all (an image alone) is read as such too
			return found || !bytes.Contains(b, cxText[1:])
		}
		rest = rest[i+len(cxType):]
		t := ""
		if j := bytes.IndexByte(rest, '"'); j >= 0 {
			t, rest = string(rest[:j]), rest[j+1:]
		}
		if t != "input_text" && t != "text" {
			continue
		}
		if !bytes.HasPrefix(rest, cxText) {
			return false
		}
		found = true
		text := strAt(rest[:len(cxText)+strEnd(rest[len(cxText):])], cxText)
		p := cxPrompt(text)
		typed = typed || p != ""
		if s.Title == "" {
			s.Title = p
		}
	}
}

// strEnd is the length of a JSON string's body and its closing quote.
func strEnd(b []byte) int {
	for j := 0; j < len(b); j++ {
		switch b[j] {
		case '\\':
			j++
		case '"':
			return j + 1
		}
	}
	return len(b)
}

// cxCounted uses the same interval reconciliation as the per-call reader.
func cxCounted(s *state, at time.Time, tot, last *cxUsage) {
	codexApply(s, codexState(s).count(at, s.Model, tot, last))
}

var (
	cxTotal = []byte(`"total_token_usage":{`)
	cxLast  = []byte(`"last_token_usage":{`)
	cxInfo  = []byte(`"info":null`)
)

// cxUsageIn reads the usage object after key in b, which holds no other
// object, without decoding the rest of the line.
func cxUsageIn(b, key []byte) (*cxUsage, bool) {
	i := bytes.Index(b, key)
	if i < 0 {
		return nil, false
	}
	obj := b[i+len(key)-1:]
	j := bytes.IndexByte(obj, '}')
	if j < 0 {
		return nil, false
	}
	var u cxUsage
	if json.Unmarshal(obj[:j+1], &u) != nil {
		return nil, false
	}
	return &u, true
}

// cxFastCount counts a token_count line from its usage objects alone; false
// when the line is laid out otherwise and wants decoding.
func cxFastCount(s *state, at time.Time, b []byte) bool {
	if h := b[:min(len(b), 1024)]; typeAfter(h, cxType) != "event_msg" || typeAfter(h, cxPayload) != "token_count" {
		return false
	}
	if bytes.Contains(b[:min(len(b), 1024)], cxInfo) {
		return true
	}
	tot, ok := cxUsageIn(b, cxTotal)
	if !ok {
		return false
	}
	last, _ := cxUsageIn(b, cxLast)
	cxCounted(s, at, tot, last)
	return true
}

// cxPrompt is the words of a prompt, or "" for what Codex put in itself
// (the environment, AGENTS.md, instructions).
func cxPrompt(text string) string {
	t := strings.TrimSpace(text)
	if t == "" || strings.HasPrefix(t, "<") || strings.HasPrefix(t, "# AGENTS.md") {
		return ""
	}
	return title(t)
}
