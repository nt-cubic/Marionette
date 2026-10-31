package gui

import (
	"bytes"
	"cmp"
	"encoding/json"
	"errors"
	"maps"
	"net/http"
	"os"
	"reflect"
	"runtime"
	"runtime/debug"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/filememo"
	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// sessionJSON is a session with what the UI needs to draw its agent.
type sessionJSON struct {
	sessions.Session
	Name string `json:"name"`
	Icon string `json:"icon"`
	// Via: the models the gateway sent its calls to, at the reasoning
	// each was asked for, when it went through magpie
	Via []usage.Via `json:"via,omitempty"`
}

type sessionsJSON struct {
	Sessions []sessionJSON `json:"sessions"`
	// Terminal is set where magpie can open Terminal on the session: the
	// Mac app, not a browser tab that may be on another computer.
	Terminal bool     `json:"terminal"`
	Dirs     []string `json:"dirs"` // where they were read from
}

// gatewayExternal is a gateway session without its days: a list, a manage
// page and a detail (MergeExternal) never read them, and copying every
// session's days for each poll of an open Sessions page was ~1.8 GB in 4
// minutes over ~720k calls (#2776). The stats add them (gatewayDays).
func gatewayExternal(g usage.GatewaySession) sessions.ExternalSession {
	e := sessions.ExternalSession{Agent: g.Agent, ID: g.ID, Start: g.Start, Last: g.Last, Tokens: g.Tokens, Cost: g.Cost, Unpriced: g.Unpriced}
	e.Models = make([]sessions.ExternalModel, 0, len(g.Models))
	for _, m := range g.Models {
		e.Models = append(e.Models, sessions.ExternalModel{Model: m.Model, Tokens: m.Tokens, Cost: m.Cost, Priced: m.Priced})
	}
	return e
}

// gatewayDays is a gateway session's usage by day, as the stats count it.
func gatewayDays(g usage.GatewaySession) []sessions.ExternalDayUsage {
	out := make([]sessions.ExternalDayUsage, 0, len(g.Daily))
	for _, d := range g.Daily {
		out = append(out, sessions.ExternalDayUsage{Date: d.Date, Agent: d.Agent, Model: d.Model, Tokens: d.Tokens, Cost: d.Cost, Priced: d.Priced})
	}
	return out
}

// gatewayListed is a gateway session as a list shows it: titled by what it
// was first asked, when magpie recorded it, not by its ses_… id (YOU on
// Discord). title is a sessions.GatewayTitler's.
func gatewayListed(g usage.GatewaySession, title func(agent, id string) string) sessions.ExternalSession {
	e := gatewayExternal(g)
	e.Title = title(g.Agent, g.ID)
	return e
}

func gatewaySession(g usage.GatewaySession) sessions.Session {
	return sessions.MergeExternal(nil, []sessions.ExternalSession{gatewayListed(g, sessions.GatewayTitle)})[0]
}

func sessionRoutes(mux *http.ServeMux, w Windows) {
	warmSessions()
	mux.HandleFunc("GET /api/sessions", func(rw http.ResponseWriter, r *http.Request) {
		// the list and the Codex threads' effort each list every session file
		defer sessions.OneListing()()
		n, _ := strconv.Atoi(r.URL.Query().Get("limit"))
		// The page draws a session count taken from every session
		// (statsFor's Overview) beside this list, so the list is read as
		// long as the count: at sessions.Limit the page said "238 sessions"
		// over a list that stopped at 200, and the agents whose sessions
		// were oldest fell off it whole — Codex had 121 and the list showed
		// 7. Only a caller that asks for a number of its own gets one. The
		// page draws this many rows in pages of its own (renderSessions).
		if n <= 0 {
			n = sessions.All
		}
		agents := map[string]*agent.Agent{}
		for _, a := range agent.Clients() {
			agents[a.ID] = a
		}
		out := sessionsJSON{Sessions: []sessionJSON{}, Terminal: runtime.GOOS == "darwin" && !isWeb(w),
			Dirs: []string{}}
		for _, d := range sessions.Dirs() {
			out.Dirs = append(out.Dirs, tilde(d))
		}
		list := sessions.List(sessions.All)
		nativeKeys := make(map[string]bool, len(list))
		for _, s := range list {
			// its copies' and subagents' ids among them
			for _, k := range sessions.GatewayKeys(s) {
				nativeKeys[k] = true
			}
		}
		gateway := listedGateway(usage.GatewaySessions(time.Time{}, nativeKeys))
		ext := make([]sessions.ExternalSession, 0, len(gateway))
		title := sessions.GatewayTitler()
		for _, g := range gateway {
			ext = append(ext, gatewayListed(g, title))
		}
		list = sessions.MergeExternal(list, ext)
		if len(list) > n {
			list = list[:n]
		}
		var modelEffort map[string][]sessions.ModelEffort
		for _, s := range list {
			if s.Agent == "codex" {
				modelEffort = usage.CodexModelEffort(usage.All)
				break
			}
		}
		for i := range list {
			addCodexModelEffort(&list[i], modelEffort)
		}
		since := time.Now()
		for _, s := range list {
			if !s.Start.IsZero() && s.Start.Before(since) {
				since = s.Start
			}
		}
		vias := usage.Vias(since.Add(-time.Minute))
		// made to size: up to sessions.All rows, grown by append each poll
		// was copied through every size on the way, ~140 MB of eight (#2278)
		out.Sessions = make([]sessionJSON, 0, len(list))
		for _, s := range list {
			// drawn as the agent it ran in: Claude Desktop's Code tab's
			// sessions are in Claude Code's folder (Session.App)
			j := sessionJSON{Session: s, Name: s.Agent, Icon: "generic", Via: viaOf(vias, s)}
			if a := agents[cmp.Or(s.App, s.Agent)]; a != nil {
				j.Name, j.Icon = a.SharedName(), a.Icon
			}
			j.Name = wslName(j.Name, s)
			j.Path = tilde(j.Path)
			out.Sessions = append(out.Sessions, j)
		}
		writeJSONRows(rw, r, "sessions", out.Sessions, struct {
			Terminal bool     `json:"terminal"`
			Dirs     []string `json:"dirs"`
		}{out.Terminal, out.Dirs})
	})
	// stats is every session's usage by day over the last ?days=N days
	// (every day when 0 or none), cut by agent, folder and model for the
	// page to filter; names are the agents' as the page shows them.
	mux.HandleFunc("GET /api/sessions/stats", func(rw http.ResponseWriter, r *http.Request) {
		defer sessions.ShareListing()()
		n, _ := strconv.Atoi(r.URL.Query().Get("days"))
		st, gen := statsForGen(n)
		names := map[string]string{}
		for _, a := range agent.Clients() {
			names[a.ID] = a.SharedName()
		}
		agents := map[string]string{}
		for _, d := range st.Days {
			for _, u := range d.Usage {
				agents[u.Agent] = cmp.Or(names[u.Agent], u.Agent)
			}
			for _, a := range d.Active {
				agents[a.Agent] = cmp.Or(names[a.Agent], a.Agent)
			}
		}
		// the page has this very answer: told so without encoding and
		// hashing a long history's ~6 MB of it again on every poll (#2278)
		statsTags.Lock()
		t, ok := statsTags.m[n]
		statsTags.Unlock()
		if ok && t.gen == gen && maps.Equal(t.agents, agents) && r.Header.Get("X-Magpie-Had") == t.tag {
			rw.Header().Set("X-Magpie-Tag", t.tag)
			rw.Header().Set("X-Magpie-Same", "1")
			rw.WriteHeader(http.StatusOK)
			return
		}
		tag := writeSame(rw, r, func(b *bytes.Buffer) error { return statsAnswer(b, st, agents) })
		if tag != "" {
			statsTags.Lock()
			if statsTags.m == nil || len(statsTags.m) >= statsMemoMost {
				statsTags.m = map[int]statsTag{}
			}
			statsTags.m[n] = statsTag{gen, agents, tag}
			statsTags.Unlock()
		}
	})
	// overview sums up the range's sessions under the page's filters:
	// how many, what the middle one spent, how many each day, and the ones
	// that spent the most.
	mux.HandleFunc("GET /api/sessions/overview", func(rw http.ResponseWriter, r *http.Request) {
		defer sessions.ShareListing()()
		q := r.URL.Query()
		n, _ := strconv.Atoi(q.Get("days"))
		st := statsFor(n)
		out := st.Overview(q.Get("agent"), q.Get("model"), q.Get("cwd"), 8)
		period := usage.All
		if n > 0 {
			if p, ok := usage.RangeOf(st.From, st.To); ok {
				period = p
			}
		}
		needCodex := false
		for key, top := range out.Top {
			out.Top[key] = slices.Clone(top)
			for i := range out.Top[key] {
				if out.Top[key][i].Agent == "codex" {
					needCodex = true
				}
			}
		}
		if needCodex {
			modelEffort := usage.CodexModelEffort(period)
			for key, top := range out.Top {
				for i := range top {
					addCodexSummaryModelEffort(&top[i], modelEffort)
				}
				out.Top[key] = top
			}
		}
		writeJSON(rw, out)
	})
	// progress is how far the reading of the session files has got, for
	// the page to show while its first read takes a while.
	mux.HandleFunc("GET /api/sessions/progress", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, sessions.Indexing())
	})
	// one is a session by its stats key, read however long ago it was at
	// work: the page's top sessions reach past the latest List reads.
	mux.HandleFunc("GET /api/sessions/one", func(rw http.ResponseWriter, r *http.Request) {
		key := r.URL.Query().Get("key")
		s, ok := sessions.Get(key)
		if !ok {
			if a, id, found := strings.Cut(key, ":"); found {
				if g, gok := usage.GatewaySessionByID(a, id, nil); gok {
					s = gatewaySession(g)
					ok = true
				}
			}
		}
		if !ok {
			rw.Header().Set("Content-Type", "application/json")
			rw.WriteHeader(http.StatusNotFound)
			_ = json.NewEncoder(rw).Encode(map[string]string{"error": "no such session"})
			return
		}
		if s.Agent == "codex" {
			addCodexModelEffort(&s, usage.CodexModelEffort(usage.All))
		}
		j := sessionJSON{Session: s, Name: s.Agent, Icon: "generic"}
		for _, a := range agent.Clients() {
			if a.ID == cmp.Or(s.App, s.Agent) {
				j.Name, j.Icon = a.SharedName(), a.Icon
			}
		}
		j.Name = wslName(j.Name, s)
		since := s.Start
		if since.IsZero() {
			since = s.Last
		}
		j.Via = viaOf(usage.Vias(since.Add(-time.Minute)), s)
		j.Path = tilde(j.Path)
		writeJSON(rw, j)
	})
	// transcript is what was said in a session, read from the agent's own
	// file (found from the session as listed, never a path from the page),
	// which is only read.
	mux.HandleFunc("GET /api/sessions/transcript", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		s, ok := sessions.Find(q.Get("agent"), q.Get("id"))
		if !ok {
			if _, found := usage.GatewaySessionByID(q.Get("agent"), q.Get("id"), nil); found {
				t, err := sessions.GatewayTranscript(q.Get("agent"), q.Get("id"))
				if err != nil {
					fail(rw, err)
					return
				}
				writeJSON(rw, t)
				return
			}
			fail(rw, errors.New("no such session"))
			return
		}
		// sub: one of its subagents, found among those the session lists
		// (#2261)
		read := sessions.TranscriptOf
		if sub := q.Get("sub"); sub != "" {
			read = func(s sessions.Session) (sessions.Transcript, error) { return sessions.SubagentTranscript(s, sub) }
		}
		t, err := read(s)
		if err != nil {
			fail(rw, err)
			return
		}
		writeJSON(rw, t)
	})
	// markdown is a session's whole conversation as a Markdown file (#1276),
	// read from the agent's own file as transcript is: to the browser that
	// asks (magpie web), which saves it itself, and, in the app, to
	// Downloads.
	mux.HandleFunc("GET /api/sessions/markdown", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		stem, md, err := sessionMarkdown(q.Get("agent"), q.Get("id"))
		if err != nil {
			fail(rw, err)
			return
		}
		rw.Header().Set("Content-Type", "text/markdown; charset=utf-8")
		rw.Header().Set("Content-Disposition", `attachment; filename="`+stem+`.md"`)
		rw.Write(md)
	})
	mux.HandleFunc("POST /api/sessions/export", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		stem, md, err := sessionMarkdown(q.Get("agent"), q.Get("id"))
		if err != nil {
			fail(rw, err)
			return
		}
		path, err := saveDownload(downloads(), stem, ".md", bytes.NewReader(md))
		if err != nil {
			fail(rw, err)
			return
		}
		writeJSON(rw, map[string]any{"path": tilde(path)})
	})
	// terminal opens Terminal on a session's resume command, or, with In, on
	// the command that carries it on in that other agent. The command is
	// made here from the session as listed, never taken from the page.
	mux.HandleFunc("POST /api/sessions/terminal", func(rw http.ResponseWriter, r *http.Request) {
		var in struct{ Agent, ID, In string }
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if runtime.GOOS != "darwin" || isWeb(w) {
			fail(rw, errors.New("opening Terminal is only for the Mac app"))
			return
		}
		s, ok := sessions.Find(in.Agent, in.ID)
		run := s.Resume
		if in.In != "" {
			run = ""
			for _, c := range s.Carry {
				if c.Agent == in.In {
					run = c.Command
				}
			}
		}
		if !ok || run == "" {
			fail(rw, errors.New("no such session"))
			return
		}
		if err := openTerminal(run, settings.Load().SessionTerminal); err != nil {
			fail(rw, err)
			return
		}
		rw.WriteHeader(http.StatusNoContent)
	})
	// carry converts a session for another agent (one its Carry offers with
	// Convert): a new session of that agent's, in its own folder, the
	// source only read. It runs only when the user picks the agent, never
	// on a listing. It answers with the line that opens the new one, and,
	// with Open, opens Terminal on it.
	mux.HandleFunc("POST /api/sessions/carry", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Agent, ID, To string
			Open          bool
		}
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if in.Open && (runtime.GOOS != "darwin" || isWeb(w)) {
			fail(rw, errors.New("opening Terminal is only for the Mac app"))
			return
		}
		s, ok := sessions.Find(in.Agent, in.ID)
		if !ok {
			fail(rw, errors.New("no such session"))
			return
		}
		c, err := sessions.CarryOver(s, in.To)
		if err != nil {
			fail(rw, err)
			return
		}
		if in.Open {
			if err := openTerminal(c.Command, settings.Load().SessionTerminal); err != nil {
				fail(rw, err)
				return
			}
		}
		writeJSON(rw, map[string]any{"agent": c.Agent, "id": c.ID, "command": c.Command})
	})
}

// sessionMarkdown is a session's conversation as Markdown, and the name its
// file is saved under: the session found as listed, never a path from the
// page. It is made whole before any of it is sent, so a file that can't be
// read is an error and not half a download.
//
// A session only magpie's gateway recorded is written from that record, as
// transcript reads it: the agent of one that reaches this magpie through a
// remote magpie ran on another machine, so there is no file of it here, and
// its download was a 400 "no such session" (akic404 on Discord).
func sessionMarkdown(agentID, id string) (stem string, md []byte, err error) {
	s, ok := sessions.Find(agentID, id)
	var t sessions.Transcript
	if !ok {
		g, found := usage.GatewaySessionByID(agentID, id, nil)
		if !found {
			return "", nil, errors.New("no such session")
		}
		if t, err = sessions.GatewayTranscript(agentID, id); err != nil {
			return "", nil, err
		}
		s = gatewaySession(g)
	}
	name := s.Agent
	for _, a := range agent.Clients() {
		if a.ID == s.Agent {
			name = a.SharedName()
		}
	}
	var b bytes.Buffer
	if ok {
		err = sessions.WriteMarkdown(&b, s, wslName(name, s))
	} else {
		err = sessions.WriteGatewayMarkdown(&b, s, name, t)
	}
	if err != nil {
		return "", nil, err
	}
	short := []rune(s.ID)
	if len(short) > 12 {
		short = short[:12]
	}
	safe := strings.Map(func(r rune) rune {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-' || r == '_' {
			return r
		}
		return '-'
	}, s.Agent+"-"+string(short))
	return "magpie-session-" + safe, b.Bytes(), nil
}

// wslName is an agent's name for a session it ran in a WSL distro, as the
// agent there is named (Claude Code · WSL Ubuntu); its own name otherwise.
func wslName(name string, s sessions.Session) string {
	if s.WSL == "" {
		return name
	}
	return name + " · WSL " + s.WSL
}

// statsFor is sessions.StatsFor for a range, kept: the page asks for the
// stats and the overview one after the other, and for the same range again
// each time it is shown. What was read is answered at once; when it is more
// than a few seconds old it is read again behind, for the next ask.
var statsMemo struct {
	sync.Mutex
	m    map[int]*statsKept
	asks uint64
}

type statsKept struct {
	at   time.Time
	st   sessions.Stats
	gen  uint64 // st's, new only when a read found st changed
	busy bool
	// used is the memo's ask count when this range was last asked: a count,
	// not a clock, since Windows' clock can give two asks the same time.
	used uint64
}

// statsMemoMost is how many distinct ?days values the memo keeps: each one is
// a whole sessions.Stats, and ?days takes any number a caller sends, so the
// least recently asked one is let go when a new value comes past it.
const statsMemoMost = 8

// statsGen numbers each Stats the memo keeps, across memos: an answer's
// tag is known by it (statsTags).
var statsGen atomic.Uint64

// statsTags is the tag of the stats answer last sent for each ?days, the
// Stats it was made from known by its gen.
var statsTags struct {
	sync.Mutex
	m map[int]statsTag
}

type statsTag struct {
	gen    uint64
	agents map[string]string
	tag    string
}

// sameStats is whether a and b would be sent the same: a read again
// behind finds the same stats most times, and keeping a's gen spares the
// page's next ask the encoding.
func sameStats(a, b sessions.Stats) bool {
	return a.From == b.From && a.To == b.To && slices.Equal(a.Keys, b.Keys) && reflect.DeepEqual(a.Days, b.Days)
}

func statsFor(days int) sessions.Stats {
	st, _ := statsForGen(days)
	return st
}

func statsForGen(days int) (sessions.Stats, uint64) {
	days = max(0, days)
	statsMemo.Lock()
	if k := statsMemo.m[days]; k != nil {
		statsMemo.asks++
		k.used = statsMemo.asks
		if !k.busy && time.Since(k.at) > 10*time.Second {
			// every range kept that is as old is read again with it, in
			// one pass: the page asks for 7, 30 and every day, and each
			// read alone listed and summed every session file (#2278)
			var ranges []int
			var kept []*statsKept
			for d, o := range statsMemo.m {
				if !o.busy && time.Since(o.at) > 10*time.Second {
					o.busy = true
					ranges, kept = append(ranges, d), append(kept, o)
				}
			}
			// read beside the page's own reads: with their listing of
			// the session files, and handed back once they all rest
			shared, rest := sessions.ShareListing(), readingPage()
			go func() {
				defer rest()
				defer shared()
				sts := combinedSessionStatsAll(ranges)
				statsMemo.Lock()
				for i, o := range kept {
					if !sameStats(o.st, sts[i]) {
						o.st, o.gen = sts[i], statsGen.Add(1)
					}
					o.at, o.busy = time.Now(), false
				}
				statsMemo.Unlock()
			}()
		}
		st, gen := k.st, k.gen
		statsMemo.Unlock()
		return st, gen
	}
	statsMemo.Unlock()
	st, gen := combinedSessionStats(days), statsGen.Add(1)
	statsMemo.Lock()
	if statsMemo.m == nil {
		statsMemo.m = map[int]*statsKept{}
	}
	if statsMemo.m[days] == nil {
		if len(statsMemo.m) >= statsMemoMost {
			oldest := -1
			for d, k := range statsMemo.m {
				if oldest == -1 || k.used < statsMemo.m[oldest].used {
					oldest = d
				}
			}
			delete(statsMemo.m, oldest)
		}
		statsMemo.asks++
		statsMemo.m[days] = &statsKept{at: time.Now(), st: st, gen: gen, used: statsMemo.asks}
	}
	statsMemo.Unlock()
	return st, gen
}

func combinedSessionStats(days int) sessions.Stats {
	return combinedSessionStatsAll([]int{days})[0]
}

// combinedSessionStatsAll is combinedSessionStats for each of days, the
// session files read once for all of them.
func combinedSessionStatsAll(days []int) []sessions.Stats {
	sts, nativeKeys := sessions.StatsAndNativeAll(days)
	for i := range sts {
		sts[i] = withGatewaySessions(sts[i], days[i], nativeKeys)
	}
	return sts
}

func withGatewaySessions(st sessions.Stats, days int, nativeKeys map[string]bool) sessions.Stats {
	var since time.Time
	if days > 0 && st.From != "" {
		since, _ = time.ParseInLocation(time.DateOnly, st.From, time.Local)
	}
	gs, gd := usage.GatewaySessionWindowExcept(since, nativeKeys)
	ext := make([]sessions.ExternalSession, 0, len(gs))
	title := sessions.GatewayTitler()
	for _, g := range gs {
		e := gatewayListed(g, title)
		e.Daily = gatewayDays(g)
		ext = append(ext, e)
	}
	daily := make([]sessions.ExternalDayUsage, 0, len(gd))
	for _, d := range gd {
		daily = append(daily, sessions.ExternalDayUsage{Date: d.Date, Agent: d.Agent, Model: d.Model, Tokens: d.Tokens, Cost: d.Cost, Priced: d.Priced})
	}
	return sessions.MergeExternalStats(st, daily, ext)
}

// warmSessions reads every session file once magpie is up, so the Sessions
// page opens on the kept index and not on a first read of them all. Usage ›
// Requests' whole history is read after it, not beside it on the disk: a
// first All parsing 12k sessions kept that page a skeleton for 40 s.
// Once a process: the app's page served to a browser (apppage.go) is a
// second Handler beside the window's.
func warmSessions() {
	if testing.Testing() {
		return
	}
	warmOnce.Do(warmStart)
}

var warmOnce sync.Once

func warmStart() {
	go func() {
		// a fresh install reads no agent's sessions until asked; one
		// that read them before keeps reading them (decided once)
		sessions.SettleReads()
		time.Sleep(3 * time.Second)
		// with few readers, as nobody waits on it yet: after an update
		// that reads every file again, it held every core (#1665)
		sessions.Warm(func() { warmUp(warmSteps()...) })
	}()
}

// warmSteps are the reads the warm-up makes before any page asks for them.
// The last is Codex's per-thread effort, when there are Codex sessions: a
// first Codex page after a start read it from the whole ledger while it
// waited (#2776), and every later ask is answered from what it kept.
func warmSteps() []func() {
	return []func(){
		func() { statsFor(0) },
		func() { statsFor(30) },
		func() { underTightGC(func() { usage.QueryPage(usage.All, usage.Filter{}, 0, 50) }) },
		func() {
			if slices.ContainsFunc(statsFor(0).Keys, func(k string) bool { return strings.HasPrefix(k, "codex:") }) {
				underTightGC(warmCodexEffort)
			}
		},
	}
}

// warmCodexEffort reads what the Sessions page's Codex rows ask for.
var warmCodexEffort = func() { usage.CodexModelEffort(usage.All) }

// warmUp runs the warm-up's reads, then hands what they threw away back to
// the system at once. Reading a long history allocates many times what it
// keeps (a 13k-session Codex and Claude Code history: ~830 MB allocated to
// keep ~80 MB of indexes), and Go's scavenger returns those pages only over
// the next minutes: a magpie that had just started read ~470 MB for its
// first two minutes at rest.
func warmUp(steps ...func()) {
	// each file read looked at once a second, not for each session or row
	// that asks: an empty sandbox's warm-up looked at settings.json ~300
	// times on Windows (#1665); a write still reads the disk (edit.Read)
	defer filememo.Hold()()
	for _, step := range steps {
		step()
	}
	debug.FreeOSMemory()
}

// underTightGC runs fn letting the heap grow by a quarter of what it holds
// before a collection, not Go's default of all of it, and then puts back
// what was set; a GOGC the user set lower, or off, is kept. The usage
// page's read of the whole history holds every row of it at once (~270 MB
// live for 390k calls), and a cold start that rests at ~200 MB peaked at
// 555–587 MB in Activity Monitor there; under it, 410–438 MB. That read
// and Codex's effort, which reads the whole ledger again while the page
// still holds its rows, run under it: the warm-up as a whole did too for
// ~11 s more CPU.
//
// It sets no memory limit. It held the memory Go asked of the system to
// what it held when fn began, and a cold start then reached the limit
// with the heap all live: from ~11 s to ~24 s it collected every ~160 ms,
// a core's worth, 3:39 of CPU in its first 60 s for 390k calls and 1166
// collections. Without it, 2:03–2:06 and 618, and the footprint's peak
// the same, 835–863 MB against 855 (#2278).
func underTightGC(fn func()) {
	defer tightenGC()()
	fn()
}

// tightGC is what the reads under a tight GC hold: the first to begin sets
// it, and the last to end puts back what was set before it. Each read
// putting back what it found let two at once leave GOGC at 25 for good,
// when the first to begin ended before the second.
var tightGC struct {
	sync.Mutex
	n   int // reads under it
	was int // GOGC before the first
}

// tightenGC begins a read under a tight GC; undo ends it.
func tightenGC() (undo func()) {
	tightGC.Lock()
	defer tightGC.Unlock()
	if tightGC.n == 0 {
		was := debug.SetGCPercent(tightGCPercent)
		if was < tightGCPercent {
			debug.SetGCPercent(was)
		}
		tightGC.was = was
	}
	tightGC.n++
	return sync.OnceFunc(func() {
		tightGC.Lock()
		defer tightGC.Unlock()
		if tightGC.n--; tightGC.n == 0 {
			debug.SetGCPercent(tightGC.was)
		}
	})
}

const tightGCPercent = 25

// openTerminal runs a command in the chosen Mac terminal through a .command
// file, as it would open from Finder: no Automation consent.
// The shell is left open when the agent quits.
func openTerminal(command, choice string) error {
	var found terminalDiscovery
	if choice != terminalBundleID {
		var err error
		// the system default falls back to Terminal when nothing is found
		if found, err = discoverTerminals(); err != nil && choice != "" && choice != "system" {
			return err
		}
	}
	args, err := terminalOpenArgs(choice, found)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp("", "magpie-resume-*.command")
	if err != nil {
		return err
	}
	script := "#!/bin/sh\n" + command + "\nexec \"${SHELL:-/bin/zsh}\" -l\n"
	_, err = f.WriteString(script)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Chmod(f.Name(), 0o700)
	}
	if err != nil {
		os.Remove(f.Name())
		return err
	}
	args = append(args, f.Name())
	if err := proc.Command("open", args...).Run(); err != nil {
		os.Remove(f.Name())
		return err
	}
	// Terminal has read it long before
	time.AfterFunc(time.Minute, func() { os.Remove(f.Name()) })
	return nil
}

// viaOf is what the gateway sent a session's calls to, under any key its
// gateway record can carry (sessions.GatewayKeys: Raft's Pi says it is Pi).
func viaOf(x usage.ViaIndex, s sessions.Session) []usage.Via {
	for _, k := range sessions.GatewayKeys(s) {
		if v := x.Of(k); v != nil {
			return v
		}
	}
	return nil
}

// statsAnswer writes the stats answer into b as an Encoder of
// struct{ sessions.Stats; Agents } does, its days one at a time: encoded
// whole, the Encoder's own buffer grew to the ~6 MB answer from nothing
// after each collection, ~200 MB of 90 s of polls while an agent worked
// (#2278).
func statsAnswer(b *bytes.Buffer, st sessions.Stats, agents map[string]string) error {
	enc := json.NewEncoder(b)
	// v as Marshal writes it: Encode ends it with a newline
	put := func(v any) error {
		if err := enc.Encode(v); err != nil {
			return err
		}
		b.Truncate(b.Len() - 1)
		return nil
	}
	b.WriteString(`{"from":`)
	if err := put(st.From); err != nil {
		return err
	}
	b.WriteString(`,"to":`)
	if err := put(st.To); err != nil {
		return err
	}
	b.WriteString(`,"days":`)
	if st.Days == nil {
		b.WriteString("null")
	} else {
		b.WriteByte('[')
		for i := range st.Days {
			if i > 0 {
				b.WriteByte(',')
			}
			if err := put(&st.Days[i]); err != nil {
				return err
			}
		}
		b.WriteByte(']')
	}
	b.WriteString(`,"keys":`)
	if err := put(st.Keys); err != nil {
		return err
	}
	b.WriteString(`,"agents":`)
	if err := put(agents); err != nil {
		return err
	}
	b.WriteString("}\n")
	return nil
}
