package usage

import (
	"slices"
	"strings"
	"sync"
	"time"
	"unsafe"

	"github.com/yetone/magpie/internal/sessions"
)

func codexIdentityRow(row Row, actor string, identities map[string]sessions.CodexIdentity) Row {
	if !isCodexRow(row) && actor == "" {
		return row
	}
	row.codexActor = actor
	row.SessionRole = "unknown"
	row.Subagent, row.ParentAgent = "", ""
	identity, ok := identities[actor]
	if actor == "" || !ok || identity.ID != actor || identity.Role != "main" && identity.Role != "subagent" {
		return row
	}
	row.SessionRole = identity.Role
	row.Session = identity.RootSession
	if identity.Role == "subagent" {
		row.Subagent = identity.ID
		if parent := identities[identity.ParentSession]; parent.ID == identity.ParentSession && parent.Role == "subagent" {
			row.ParentAgent = parent.ID
		}
	}
	return row
}

func codexActor(call sessions.Call) string {
	if call.Agent == "codex" {
		return call.ThreadID
	}
	return ""
}

func codexResponseMatches(records []Record, calls []sessions.Call, duplicateCalls map[int]bool) map[int]int {
	recordIDs := map[string][]int{}
	for i, record := range records {
		if record.ResponseID != "" {
			recordIDs[record.ResponseID] = append(recordIDs[record.ResponseID], i)
		}
	}
	callIDs := map[string][]int{}
	for i, call := range calls {
		if call.ResponseID != "" && !duplicateCalls[i] {
			callIDs[call.ResponseID] = append(callIDs[call.ResponseID], i)
		}
	}
	pairs := map[int]int{}
	for id, recordRefs := range recordIDs {
		callRefs := callIDs[id]
		if len(recordRefs) != 1 || len(callRefs) != 1 {
			continue
		}
		record, call := recordRefs[0], callRefs[0]
		if records[record].Agent == "codex" && !records[record].IsRejected() && records[record].Source != SourceReported && calls[call].Agent == "codex" {
			pairs[record] = call
		}
	}
	return pairs
}

func sameCodexCall(a, b Row) bool {
	return isCodexRow(a) && isCodexRow(b) && a.ResponseID == b.ResponseID && a.Time.Equal(b.Time) &&
		a.Model == b.Model && a.Requested == b.Requested && a.Served == b.Served && a.Effort == b.Effort &&
		a.Input == b.Input && a.Output == b.Output && a.CacheRead == b.CacheRead && a.CacheWrite == b.CacheWrite &&
		a.CacheWrite1h == b.CacheWrite1h && a.Reasoning == b.Reasoning && a.Millis == b.Millis && a.TTFT == b.TTFT &&
		a.Error == b.Error && a.ErrType == b.ErrType && a.RequestID == b.RequestID
}

// codexCompactionOf is what a native Codex call says about compaction: only
// a call read from a token_usage_record (it has a response_id) can say, as
// Codex writes that record and a compaction's compaction_response_id together
// (openai/codex#41912). An older rollout's call, told from token_count alone,
// never carries the marker, so it is unknown, not "no compaction".
func codexCompactionOf(call sessions.Call) *bool {
	if call.Agent != "codex" || call.ResponseID == "" {
		return nil
	}
	compacted := call.Compaction
	return &compacted
}

func mergeCodexCompactions(values []*bool) *bool {
	allKnown, confirmed := len(values) > 0, false
	for _, value := range values {
		if value == nil {
			allKnown = false
			continue
		}
		confirmed = confirmed || *value
	}
	if confirmed {
		yes := true
		return &yes
	}
	if allKnown {
		no := false
		return &no
	}
	return nil
}

func isCodexRow(row Row) bool {
	if row.Client != "" {
		return row.Client == "codex"
	}
	return row.Agent == "codex"
}

func duplicateCodexCalls(calls []sessions.Call) (map[int]bool, map[int]string, map[int]*bool) {
	local := map[string][]int{}
	for i, call := range calls {
		if call.Agent == "codex" && call.ResponseID != "" {
			local[call.ResponseID] = append(local[call.ResponseID], i)
		}
	}
	skip, actors, compactions := map[int]bool{}, map[int]string{}, map[int]*bool{}
	for index, call := range calls {
		if call.Agent == "codex" {
			compactions[index] = codexCompactionOf(call)
		}
	}
	for _, indexes := range local {
		if len(indexes) < 2 {
			continue
		}
		rows := make([]Row, len(indexes))
		actor := calls[indexes[0]].ThreadID
		actorConflict := actor == ""
		identical := true
		for i, index := range indexes {
			rows[i] = Row{Record: logRecord(calls[index])}
			if calls[index].ThreadID != actor {
				actorConflict = true
			}
			if i > 0 && !sameCodexCall(rows[0], rows[i]) {
				identical = false
			}
		}
		if !identical {
			for _, index := range indexes {
				actors[index] = ""
			}
			continue
		}
		for _, index := range indexes[1:] {
			skip[index] = true
		}
		merged := make([]*bool, len(indexes))
		for i, index := range indexes {
			merged[i] = codexCompactionOf(calls[index])
		}
		compactions[indexes[0]] = mergeCodexCompactions(merged)
		if actorConflict {
			actors[indexes[0]] = ""
		} else {
			actors[indexes[0]] = actor
		}
	}
	return skip, actors, compactions
}

func cloneCodexChunks(chunks []*rowChunk) []*rowChunk {
	out := make([]*rowChunk, len(chunks))
	for i, chunk := range chunks {
		copy := *chunk
		copy.codexActors = slices.Clone(chunk.codexActors)
		copy.codexCompactions = nil
		copy.codexIdentities = nil
		out[i] = &copy
	}
	return out
}

func codexChunkResponsePairs(gateways, locals []*rowChunk, duplicateLocal map[rowRef]bool) map[rowRef]rowRef {
	gatewayIDs := map[string][]rowRef{}
	for _, chunk := range gateways {
		for i := range chunk.Rows {
			row := chunk.row(i)
			if row.ResponseID != "" {
				gatewayIDs[row.ResponseID] = append(gatewayIDs[row.ResponseID], rowRef{chunk, i})
			}
		}
	}
	localIDs := map[string][]rowRef{}
	for _, chunk := range locals {
		for i := range chunk.Rows {
			ref := rowRef{chunk, i}
			if duplicateLocal[ref] {
				continue
			}
			row := chunk.row(i)
			if row.ResponseID != "" {
				localIDs[row.ResponseID] = append(localIDs[row.ResponseID], ref)
			}
		}
	}
	pairs := map[rowRef]rowRef{}
	for id, gatewayRefs := range gatewayIDs {
		localRefs := localIDs[id]
		if len(gatewayRefs) != 1 || len(localRefs) != 1 {
			continue
		}
		gateway, local := gatewayRefs[0], localRefs[0]
		gatewayRow, localRow := gateway.row(), local.row()
		if isCodexRow(gatewayRow) && !gatewayRow.IsRejected() && gatewayRow.Source != SourceReported && isCodexRow(localRow) && localRow.Source == "log" {
			pairs[gateway] = local
		}
	}
	return pairs
}

func foldCodexRows(locals []*rowChunk) map[rowRef]bool {
	localIDs := map[string][]rowRef{}
	for _, chunk := range locals {
		for i := range chunk.Rows {
			row := chunk.row(i)
			if isCodexRow(row) && row.Source == "log" && row.ResponseID != "" {
				localIDs[row.ResponseID] = append(localIDs[row.ResponseID], rowRef{chunk, i})
			}
		}
	}
	skip := map[rowRef]bool{}
	for _, refs := range localIDs {
		if len(refs) < 2 {
			continue
		}
		actors := make([]string, len(refs))
		rows := make([]Row, len(refs))
		identical := true
		actor := ""
		actorConflict := false
		for i, ref := range refs {
			rows[i] = ref.row()
			if ref.Index < len(ref.Chunk.codexActors) {
				actors[i] = ref.Chunk.codexActors[ref.Index]
			}
			if i == 0 {
				actor = actors[i]
			} else {
				if actors[i] != actor {
					actorConflict = true
				}
				if !sameCodexCall(rows[0], rows[i]) {
					identical = false
				}
			}
		}
		if !identical {
			for _, ref := range refs {
				setCodexActor(ref.Chunk, ref.Index, "")
			}
			continue
		}
		if actorConflict {
			setCodexActor(refs[0].Chunk, refs[0].Index, "")
		}
		for _, ref := range refs[1:] {
			skip[ref] = true
		}
		compactions := make([]*bool, len(refs))
		for i := range refs {
			compactions[i] = rows[i].Compaction
		}
		refs[0].Chunk.setCodexCompaction(refs[0].Index, mergeCodexCompactions(compactions))
	}
	return skip
}

func setCodexActor(chunk *rowChunk, index int, actor string) {
	if len(chunk.codexActors) != len(chunk.Rows) {
		actors := make([]string, len(chunk.Rows))
		copy(actors, chunk.codexActors)
		chunk.codexActors = actors
	}
	chunk.codexActors[index] = actor
}

func codexActorBytes(actors []string) int64 {
	bytes := int64(cap(actors)) * int64(unsafe.Sizeof(""))
	for _, actor := range actors {
		bytes += int64(len(actor))
	}
	return bytes
}

// codexEffort keeps CodexModelEffort's last answer for each period: the
// Sessions page asks for all history every 15s, and the ledger under it is
// read again only when a call, a price or an identity changed.
//
// A gateway in use writes a call every few seconds, so the answer is often
// out of date when it is asked for. Only one ledger is built at a time, and
// while one is built for a period that has an answer already, that answer is
// what is said: each request used to build its own ledger of all history
// (#2278, with 68k session files and 75k calls a week), and the Sessions
// page's requests arriving together grew the app to 85 GB.
var codexEffort = struct {
	sync.Mutex
	periods map[Period]*effortEntry
	built   int64 // a clock for which period was asked for last
}{periods: map[Period]*effortEntry{}}

// codexEffortBuilding holds one ledger build at a time.
var codexEffortBuilding sync.Mutex

// effortPeriodsKept is how many periods' answers are kept: a picked range is
// a period of its own, so they are not kept without end.
const effortPeriodsKept = 8

type effortEntry struct {
	key      string
	root     string // the logs it was read from: another home's is no answer
	value    map[string][]sessions.ModelEffort
	have     bool
	building chan struct{} // closed when the build under way ends
	used     int64
}

// effortBuilt, in tests, is told each time CodexModelEffort reads the ledger.
var effortBuilt func()

// CodexModelEffort reports each Codex thread's own priced ledger consumption.
func CodexModelEffort(p Period) map[string][]sessions.ModelEffort {
	if LogCalls != nil { // synthetic logs carry no state to key on
		return codexModelEffort(LedgerOf(p, Filter{}).Rows)
	}
	now := Clock()
	st := readRequestState(now)
	codexEffort.Lock()
	e := codexEffort.periods[p]
	if e == nil {
		e = &effortEntry{}
		codexEffort.periods[p] = e
		forgetEffortPeriods(p)
	}
	codexEffort.built++
	e.used = codexEffort.built
	if e.have && (e.key == st.key || e.building != nil && e.root == st.root) {
		value := e.value // current, or the last one while the next is built
		codexEffort.Unlock()
		return cloneEffort(value)
	}
	if wait := e.building; wait != nil { // the first answer is on its way
		codexEffort.Unlock()
		<-wait
		codexEffort.Lock()
		value := e.value
		codexEffort.Unlock()
		return cloneEffort(value)
	}
	done := make(chan struct{})
	e.building = done
	codexEffort.Unlock()
	var value map[string][]sessions.ModelEffort
	defer func() {
		codexEffort.Lock()
		e.building = nil
		if value != nil {
			// kept under the state read before the build: a call written
			// while it ran changes the state, and the next ask builds again
			e.key, e.root, e.value, e.have = st.key, st.root, value, true
		}
		codexEffort.Unlock()
		close(done)
	}()
	codexEffortBuilding.Lock()
	defer codexEffortBuilding.Unlock()
	if effortBuilt != nil {
		effortBuilt()
	}
	// Only Codex's own calls carry a Codex thread: a gateway call is told
	// its thread by the Codex call that has its response id, so the other
	// agents' session files are not read for this.
	value = codexModelEffort(ledgerOfAt(p, Filter{}, now, codexCalls).Rows)
	return cloneEffort(value)
}

func codexCalls(since time.Time) []sessions.Call { return sessions.AgentCalls(since, "codex") }

// forgetEffortPeriods drops the answers asked for longest ago, keeping p's
// entry; codexEffort is held.
func forgetEffortPeriods(p Period) {
	for len(codexEffort.periods) > effortPeriodsKept {
		var oldest Period
		var at int64 = -1
		for q, e := range codexEffort.periods {
			if q != p && e.building == nil && (at < 0 || e.used < at) {
				oldest, at = q, e.used
			}
		}
		if at < 0 {
			return
		}
		delete(codexEffort.periods, oldest)
	}
}

func cloneEffort(values map[string][]sessions.ModelEffort) map[string][]sessions.ModelEffort {
	out := make(map[string][]sessions.ModelEffort, len(values))
	for actor, v := range values {
		out[actor] = slices.Clone(v)
	}
	return out
}

func codexModelEffort(rows []Row) map[string][]sessions.ModelEffort {
	type modelEffortKey struct{ model, effort string }
	type total struct {
		value  sessions.ModelEffort
		priced bool
	}
	groups := map[string]map[modelEffortKey]*total{}
	for _, row := range rows {
		if row.codexActor == "" || row.IsRejected() ||
			row.Input+row.Output+row.CacheRead+row.CacheWrite == 0 {
			continue
		}
		byModel := groups[row.codexActor]
		if byModel == nil {
			byModel = map[modelEffortKey]*total{}
			groups[row.codexActor] = byModel
		}
		key := modelEffortKey{row.Model, row.Effort}
		entry := byModel[key]
		if entry == nil {
			entry = &total{value: sessions.ModelEffort{Model: row.Model, Effort: row.Effort}, priced: true}
			byModel[key] = entry
		}
		entry.priced = entry.priced && row.Priced
		entry.value.Input += row.Input
		entry.value.Output += row.Output
		entry.value.CacheRead += row.CacheRead
		entry.value.CacheWrite += row.CacheWrite
		entry.value.CacheWrite1h += row.CacheWrite1h
		entry.value.Reasoning += row.Reasoning
		entry.value.Cost += row.Cost
	}
	out := make(map[string][]sessions.ModelEffort, len(groups))
	for actor, byModel := range groups {
		values := make([]sessions.ModelEffort, 0, len(byModel))
		for _, entry := range byModel {
			entry.value.Priced = entry.priced
			values = append(values, entry.value)
		}
		slices.SortFunc(values, func(a, b sessions.ModelEffort) int {
			if a.Model != b.Model {
				return strings.Compare(a.Model, b.Model)
			}
			return strings.Compare(a.Effort, b.Effort)
		})
		out[actor] = values
	}
	return out
}
