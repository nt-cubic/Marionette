package usage

// The request page keeps compact per-source records, not a second full ledger.
// Unchanged sources are never decoded again. Aggregation streams those records;
// only the requested page is materialized/sorted. Export can still ask for the
// complete ledger explicitly. This cache is disposable and bounded.

import (
	"container/heap"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"sync"
	"time"
	"unsafe"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/settings"
)

type RequestPage struct {
	Rows              []Row
	Sum               Totals
	Total             int
	Agents, Providers []string
	Purposes          []string // all purposes in the period, independent of the selected filter
	CallerKeys        []Group
	// Through and Direct are the rows the gateway served and the rows read
	// from the agents' own session files, which the gateway never saw. Sum
	// is the two together; the page shows the three beside each other so a
	// reader can tell what magpie carried from what it only read about.
	Through, Direct Totals
	// Reported are the calls a client told magpie of (#1874), which went
	// straight to their vendor: counted in Sum, apart from both of those.
	Reported Totals
	// Accounts are the subscription accounts that answered calls in the
	// period, by provider and account, for the Account filter (#557)
	Accounts []Group
	Bucket   string
	Series   []SeriesPoint
	By       map[string][]Share
	ChartBy  map[string][]Share // the whole period's legend while a day is selected
	// Computers are the rows told apart by the computer they were made on,
	// ThisComputer's and each other's by id, of the rows without the
	// filter's computer, and Names what the others are called: none when no
	// other computer's calls were brought here by sync (#542)
	Computers []Share
	Names     map[string]string
	// Heat is the rows by day, as the chart's filter keeps them: only for
	// the heatmap's own period (HeatmapOf)
	Heat *Heatmap
}

type packedRow struct {
	Time time.Time
	// Varied are the strings of rowVaried, and Shape the chunk's Shapes
	// entry that holds the row's other strings
	Varied                         [len(rowVaried)]uint32
	Shape                          uint32
	Tokens                         [6]int64
	Millis, TTFT, FirstText, Order int64
	Sent, Flow, FirstReasoning     int64
	Own, Retried, Queued           int64
	RouteID                        int64
	Cost, Credits                  float64
	Status, Tries                  int32
	Flags                          uint16
}

// rowHandedOn is the Flags bit of Record.HandedOn, rowReasoningHidden
// Record.ReasoningHidden's: 64 is block_price.go's swapKnown, 256
// rowOwnCost
const (
	rowHandedOn        = 128
	rowReasoningHidden = 512
	rowCompactionKnown = 1024
	rowCompaction      = 2048
)

// rowOwnCost marks a reported call that said what it cost (#1874): Cost is
// that, which the block pricer keeps, and reading the row back gives
// Record.OwnCost again.
const rowOwnCost = 256

// rowCredits marks a call whose reply said what it cost in its plan's
// credits (#2512): Credits is that, and reading the row back gives
// Record.Credits again.
const rowCredits = 4096

type rowChunk struct {
	Archive   []byte
	Locations []*time.Location
	Rows      []packedRow
	Strings   []string
	// Shapes are the rows' strings but rowVaried's, each set kept once:
	// in every row they were 140 of its 328 bytes, and the page holds every
	// row of the period at once. 364k calls had 7300 shapes.
	Shapes           [][rowTexts]uint32
	shapeIDs         map[[rowTexts]uint32]uint32
	codexActors      []string
	codexCompactions map[int]*bool
	codexIdentities  map[string]sessions.CodexIdentity
	dict             map[string]uint32
	Source           sessions.CallSource
	Bytes            int64
	Used             uint64
	Count            int
	Latest           time.Time
	// Computer is the other computer whose day this is (#542), by id
	Computer string
}

// rowMsg is the Text of a row's Claude message id, after rowText's, and
// rowError its error's
const (
	rowMsg   = 34
	rowError = 9
	// rowClient is the Text of the client's own name, when the user counts
	// it as another agent (Row.Client, #1958)
	rowClient = 33
	// rowTexts is how many strings a row has: rowText's and the message id
	rowTexts = 35
)

// rowVaried are the strings that differ call by call: the error, the
// request and response ids, the archive and the message id. A row keeps
// them itself; every other string is the same for most of a chunk's rows.
var rowVaried = [...]int{rowError, 11, 22, 25, rowMsg}

// rowVariedAt is each Text slot's place in Varied, or -1
var rowVariedAt = func() (at [rowTexts]int8) {
	for i := range at {
		at[i] = -1
	}
	for k, slot := range rowVaried {
		at[slot] = int8(k)
	}
	return at
}()

// text is the string id of p's Text slot
func (c *rowChunk) text(p *packedRow, slot int) uint32 {
	if k := rowVariedAt[slot]; k >= 0 {
		return p.Varied[k]
	}
	return c.Shapes[p.Shape][slot]
}

// texts is every string id of p, by Text slot
func (c *rowChunk) texts(p *packedRow) [rowTexts]uint32 {
	t := c.Shapes[p.Shape]
	for k, slot := range rowVaried {
		t[slot] = p.Varied[k]
	}
	return t
}

// setTexts keeps t, every string id of p by Text slot, in p and Shapes
func (c *rowChunk) setTexts(p *packedRow, t [rowTexts]uint32) {
	for k, slot := range rowVaried {
		p.Varied[k], t[slot] = t[slot], 0
	}
	p.Shape = c.shape(t)
}

// shape is the Shapes entry of t, which is added when there is none
func (c *rowChunk) shape(t [rowTexts]uint32) uint32 {
	if c.shapeIDs == nil {
		c.shapeIDs = make(map[[rowTexts]uint32]uint32, len(c.Shapes))
		for i, s := range c.Shapes {
			c.shapeIDs[s] = uint32(i)
		}
	}
	id, ok := c.shapeIDs[t]
	if !ok {
		id = uint32(len(c.Shapes))
		c.Shapes = append(c.Shapes, t)
		c.shapeIDs[t] = id
		c.Bytes += int64(unsafe.Sizeof(t))
	}
	return id
}

// rowText is every string of a row's record, the upstream's own stop
// reason ("Upstream stop reason", #1927) and the service tiers (#1784)
// among them: a string left out comes back empty from the cache, in the
// page and in /v1/magpie/usage/requests (TestRowTextHoldsEveryString)
func rowText(r *Row) [34]*string {
	return [34]*string{&r.Agent, &r.Provider, &r.Host, &r.SessionProvider, &r.SessionAccount, &r.Model, &r.Requested, &r.Served, &r.Effort, &r.Error, &r.ErrType, &r.RequestID, &r.Endpoint, &r.Session, &r.NativeSession, &r.Kind, &r.Source, &r.Via, &r.ProviderKeyID, &r.ProviderKeyName, &r.CallerKeyID, &r.CallerKeyName, &r.Archive, &r.Operation, &r.ProviderAccount, &r.ResponseID, &r.Upstream, &r.Subagent, &r.ParentAgent, &r.Stop, &r.TierAsked, &r.TierSent, &r.TierServed, &r.Client}
}
func (c *rowChunk) add(r Row, msg string, order int64, failed bool) {
	if c.dict == nil {
		c.dict = map[string]uint32{"": 0}
		c.Strings = []string{""}
	}
	intern := func(s string) uint32 {
		if id, ok := c.dict[s]; ok {
			return id
		}
		id := uint32(len(c.Strings))
		c.dict[s] = id
		c.Strings = append(c.Strings, s)
		c.Bytes += int64(len(s) + 48)
		return id
	}
	p := packedRow{Time: r.Time, Tokens: [6]int64{int64(r.Input), int64(r.Output), int64(r.CacheRead), int64(r.CacheWrite), int64(r.Reasoning), int64(r.CacheWrite1h)}, Millis: r.Millis, TTFT: r.TTFT, FirstText: r.FirstText, FirstReasoning: r.FirstReasoning, Sent: r.Sent, Flow: r.Flow, Own: r.Own, Retried: r.Retried, Queued: r.Queued, Order: order, RouteID: r.RouteID, Cost: r.Cost, Status: int32(r.Status), Tries: int32(r.Tries)}
	var t [rowTexts]uint32
	for i, s := range rowText(&r) {
		t[i] = intern(*s)
	}
	t[rowMsg] = intern(msg)
	c.setTexts(&p, t)
	if r.Priced {
		p.Flags |= 1
	}
	if r.Swapped {
		p.Flags |= 2
	}
	if r.Rejected {
		p.Flags |= 4
	}
	if r.SessionOfficialLogin {
		p.Flags |= 8
	}
	if r.ReasoningHidden {
		p.Flags |= rowReasoningHidden
	}
	if r.Routed {
		p.Flags |= 32
	}
	if failed {
		p.Flags |= 16
	}
	if r.HandedOn {
		p.Flags |= rowHandedOn
	}
	if r.OwnCost != nil {
		p.Flags |= rowOwnCost
		p.Cost = *r.OwnCost
	}
	if r.Credits != nil {
		p.Flags |= rowCredits
		p.Credits = *r.Credits
	}
	if r.Compaction != nil {
		p.Flags |= rowCompactionKnown
		if *r.Compaction {
			p.Flags |= rowCompaction
		}
	}
	c.Rows = append(c.Rows, p)
	if c.codexActors != nil || r.codexActor != "" {
		if c.codexActors == nil {
			c.codexActors = make([]string, len(c.Rows)-1)
			c.Bytes += int64(len(c.codexActors)) * int64(unsafe.Sizeof(""))
		}
		c.codexActors = append(c.codexActors, r.codexActor)
	}
	c.Count = len(c.Rows)
	if r.Time.After(c.Latest) {
		c.Latest = r.Time
	}
	c.Bytes += int64(unsafe.Sizeof(packedRow{}))
	if c.codexActors != nil {
		c.Bytes += int64(unsafe.Sizeof("")) + int64(len(r.codexActor))
	}
}
func (c *rowChunk) row(i int) Row {
	p := &c.Rows[i]
	r := Row{Record: Record{RouteID: p.RouteID, Time: p.Time, Input: int(p.Tokens[0]), Output: int(p.Tokens[1]), CacheRead: int(p.Tokens[2]), CacheWrite: int(p.Tokens[3]), CacheWrite1h: int(p.Tokens[5]), Reasoning: int(p.Tokens[4]), Millis: p.Millis, TTFT: p.TTFT, FirstText: p.FirstText, FirstReasoning: p.FirstReasoning, ReasoningHidden: p.Flags&rowReasoningHidden != 0, Sent: p.Sent, Flow: p.Flow, Own: p.Own, Retried: p.Retried, Queued: p.Queued, Status: int(p.Status), Tries: int(p.Tries), HandedOn: p.Flags&rowHandedOn != 0, Rejected: p.Flags&4 != 0, SessionOfficialLogin: p.Flags&8 != 0}, Cost: p.Cost, Priced: p.Flags&1 != 0, Swapped: p.Flags&2 != 0, Routed: p.Flags&32 != 0}
	if p.Flags&rowCredits != 0 {
		credits := p.Credits
		r.Credits = &credits
	}
	if p.Flags&rowCompactionKnown != 0 {
		compacted := p.Flags&rowCompaction != 0
		r.Compaction = &compacted
	}
	if compacted, ok := c.codexCompactions[i]; ok {
		if compacted == nil {
			r.Compaction = nil
		} else {
			value := *compacted
			r.Compaction = &value
		}
	}
	t := c.texts(p)
	for i, s := range rowText(&r) {
		*s = c.Strings[t[i]]
	}
	if r.Swapped && p.Flags&swapKnown == 0 && (SameSpelled(r.Model, r.Served) || ServingName(r.Model, r.Served) || RelayRenamed(r.Model, r.Served)) {
		r.Swapped = false // kept before a name spelled otherwise, a vendor's serving name or a relay's rename was the same
	}
	if r.Source == SourceReported {
		r.Record.Source = SourceReported
		if p.Flags&rowOwnCost != 0 {
			own := p.Cost
			r.OwnCost = &own
		}
	}
	r.Computer = c.Computer
	if c.codexIdentities != nil {
		actor := ""
		if i < len(c.codexActors) {
			actor = c.codexActors[i]
		}
		r = codexIdentityRow(r, actor, c.codexIdentities)
	}
	return r
}

func (c *rowChunk) setCodexCompaction(index int, compacted *bool) {
	if c.codexCompactions == nil {
		c.codexCompactions = make(map[int]*bool)
	}
	if compacted == nil {
		c.codexCompactions[index] = nil
		return
	}
	value := *compacted
	c.codexCompactions[index] = &value
}

type rowRef struct {
	Chunk *rowChunk
	Index int
}

func (r rowRef) row() Row      { return r.Chunk.row(r.Index) }
func (r rowRef) at() time.Time { return r.Chunk.Rows[r.Index].Time }
func refNewer(a, b rowRef) bool {
	if !a.at().Equal(b.at()) {
		return a.at().After(b.at())
	}
	if (a.Chunk.Source.Path == "") != (b.Chunk.Source.Path == "") {
		return a.Chunk.Source.Path == ""
	}
	if a.Chunk.Source.Path != b.Chunk.Source.Path {
		return a.Chunk.Source.Path > b.Chunk.Source.Path
	}
	return a.Chunk.Rows[a.Index].Order > b.Chunk.Rows[b.Index].Order
}

type newestHeap []rowRef

func (h newestHeap) Len() int           { return len(h) }
func (h newestHeap) Less(i, j int) bool { return refNewer(h[j], h[i]) }
func (h newestHeap) Swap(i, j int)      { h[i], h[j] = h[j], h[i] }
func (h *newestHeap) Push(v any)        { *h = append(*h, v.(rowRef)) }
func (h *newestHeap) Pop() any          { a := *h; v := a[len(a)-1]; *h = a[:len(a)-1]; return v }

type pageKey struct {
	Period        Period
	Filter        Filter
	Offset, Limit int
	// Date is today in the reader's zone (Filter.Zone): the request
	// state's key turns with this computer's day, and a reader in another
	// zone has a today of their own
	Date string
}
type requestIndex struct {
	sync.Mutex
	root, meta, key string
	chunks          map[string]*rowChunk
	pages           map[pageKey]RequestPage
	tick            uint64
	// flights are the pages being worked out: a second ask for one waits
	// for it rather than reading the whole history beside it (a refresh
	// pressed while a 750k-call All was still being read, Vantiboo1ean)
	flights map[flightKey]*pageFlight
}

// building, in tests, is told each time a page is worked out.
var building func()

type flightKey struct {
	key string
	q   pageKey
}
type pageFlight struct {
	done chan struct{}
	page RequestPage
}

var requestCache requestIndex

var requestCacheBytes int64 = 24 << 20

func statKey(path string) string {
	s, e := os.Stat(path)
	if e != nil {
		return path + ":missing"
	}
	return fmt.Sprintf("%s:%d:%d", path, s.Size(), s.ModTime().UnixNano())
}
func recordHash(path string, n int64) string {
	f, e := os.Open(path)
	if e != nil {
		return ""
	}
	defer f.Close()
	h := sha256.New()
	if _, e = io.CopyN(h, f, n); e != nil {
		return ""
	}
	return fmt.Sprintf("%x", h.Sum(nil))
}

// requestState is what the calls read at now depend on: the gateway log's
// snapshot, the agents' call files, other computers' days, Codex's thread
// identities and what prices and names a call. A change to meta drops the
// request index's chunks; a change to key, its pages.
type requestState struct {
	sources         []sessions.CallSource
	codexIdentities map[string]sessions.CodexIdentity
	cfg             settings.Settings
	snapshot        *logSnapshot
	days            []sharedSource
	someShared      bool
	root, meta, key string
}

func readRequestState(now time.Time) requestState {
	sources := sessions.CallSources()
	codexIdentities := sessions.CodexIdentities()
	slices.SortFunc(sources, func(a, b sessions.CallSource) int {
		if n := a.Modified.Compare(b.Modified); n != 0 {
			return n
		}
		return strings.Compare(a.Path, b.Path)
	})
	ids, _ := json.Marshal(provider.SessionIdentities(sessions.CodexDir()))
	codexIdentityKey, _ := json.Marshal(codexIdentities)
	renamed, _ := json.Marshal(provider.Renamed())
	cfg := settings.Load()
	prices, _ := json.Marshal(cfg.ModelPrices)
	wires, _ := json.Marshal(cfg.ModelWires)
	meta := fmt.Sprintf("%s|%s|%s|%s|%s|%s|%s|%s", ids, codexIdentityKey, renamed, prices, wires, statKey(provider.Path()), statKey(catalog.CachePath()), clientAgentsKey())
	h := sha256.New()
	for _, current := range provider.All() {
		fmt.Fprintf(h, "%q:%q:%v:%t:%t:%t;", current.ID, current.Where(), current.Catalogs(), current.IsPlugin(), current.Account == nil, current.Key != "")
		fmt.Fprint(h, statKey(catalog.LivePath(current.ID)))
	}
	for _, root := range sessions.DesktopDataDirs() {
		for _, kind := range []string{"local-agent-mode-sessions", "claude-code-sessions"} {
			files, _ := sessions.SessionGlob(filepath.Join(root, kind, "*", "*", "local_*.json"))
			for _, path := range files {
				fmt.Fprint(h, statKey(path))
			}
		}
	}
	meta += fmt.Sprintf("|%x|%s", h.Sum(nil), statKey(catalog.LivePath("antigravity")))
	h.Reset()
	snapshot := readLogSnapshot()
	fmt.Fprint(h, meta, snapshot.version, now.In(time.Local).Format("2006-01-02 MST"))
	for _, s := range sources {
		fmt.Fprintf(h, "%s:%d:%d;", s.Path, s.Size, s.Modified.UnixNano())
	}
	days, someShared := sharedSources(time.Time{})
	for _, s := range days {
		fmt.Fprintf(h, "%s:%d:%d;", s.path, s.size, s.mod.UnixNano())
	}
	return requestState{sources: sources, codexIdentities: codexIdentities, cfg: cfg, snapshot: snapshot, days: days, someShared: someShared,
		root: Path() + "|" + catalog.CachePath(), meta: meta, key: fmt.Sprintf("%x", h.Sum(nil))}
}

func QueryPage(p Period, f Filter, offset, limit int) RequestPage {
	return queryPage(p, f, offset, limit, sessions.ReadCallSource)
}
func queryPage(p Period, f Filter, offset, limit int, readSource func(sessions.CallSource) []sessions.Call) (page RequestPage) {
	if limit <= 0 {
		limit = 100
	}
	limit = min(limit, 500)
	offset = max(0, offset)
	// The time is read once, so that the rows, the chart and the day the page
	// is kept for are of one day when midnight falls while it is worked out,
	// and read in the reader's zone, whose days and hours the page shows.
	now := Clock().In(f.loc())
	// A replacement reader remains useful to callers supplying synthetic logs.
	if LogCalls != nil {
		return pageFromLedgerAt(p, f, offset, limit, LedgerOfAt(p, Filter{}, now), now)
	}
	st := readRequestState(now)
	sources, codexIdentities, cfg, snapshot, days, someShared := st.sources, st.codexIdentities, st.cfg, st.snapshot, st.days, st.someShared
	version, meta, key, root := snapshot.version, st.meta, st.key, st.root
	idx := &requestCache
	idx.Lock()

	if idx.root != root || idx.meta != meta {
		idx.root, idx.meta = root, meta
		idx.chunks = map[string]*rowChunk{}
		idx.pages = nil
		idx.key = ""
	}
	if idx.key != key {
		idx.pages = map[pageKey]RequestPage{}
		idx.key = key
	}
	q := pageKey{p, f, offset, limit, now.Format(time.DateOnly)}
	if page, ok := idx.pages[q]; ok {
		idx.Unlock()
		return page
	}
	fk := flightKey{key, q}
	if fl := idx.flights[fk]; fl != nil {
		idx.Unlock()
		<-fl.done
		return fl.page
	}
	if idx.flights == nil {
		idx.flights = map[flightKey]*pageFlight{}
	}
	fl := &pageFlight{done: make(chan struct{})}
	idx.flights[fk] = fl
	defer func() {
		requestCache.Lock()
		if requestCache.flights[fk] == fl {
			delete(requestCache.flights, fk)
		}
		requestCache.Unlock()
		fl.page = page
		close(fl.done)
	}()
	idx.tick++
	// Copy only metadata while holding the lock. Published chunks are immutable;
	// reading, decoding, pricing and aggregation use a private snapshot.
	shared := idx
	idx = &requestIndex{root: idx.root, meta: idx.meta, key: idx.key, tick: idx.tick,
		chunks: maps.Clone(idx.chunks)}
	shared.Unlock()
	price := Pricer()
	priceRow := func(r Record, source string) Row {
		sent := r.Model
		if source == "" {
			sent = provider.SentNameIn(cfg.ModelWires, r.Provider, r.Model, r.Effort)
		}
		row := Row{Record: r, Source: source, Swapped: r.Served != "" && Swapped(sent, r.Served), Routed: GroupRouted(sent, r.Served)}
		if pr := price(r); pr != nil && r.Input+r.Output > 0 {
			row.Priced = true
			row.Cost = r.CostAt(*pr)
		}
		row.Agent, row.Client = reportedAs(r.Agent)
		return row
	}
	since := p.Since(now)
	gatewaySince := since
	if !since.IsZero() {
		gatewaySince = since.Add(-24 * time.Hour)
	}
	// The gateway's blocks are priced as they are read, from a memo of each
	// provider, model, served model and effort the block names: a priced
	// copy of each block kept beside it doubled the index, and over the
	// cache budget every query priced the whole history again.
	var gateways []*rowChunk
	priced := newBlockPricer(provider.Renamed(), cfg.ModelWires, price)
	for _, packed := range snapshot.blocks {
		if !gatewaySince.IsZero() && packed.Latest.Before(gatewaySince) {
			continue
		}
		if c := priced.block(packed); c != nil {
			gateways = append(gateways, c)
		}
	}
	on := map[string]bool{}
	var chunks []*rowChunk
	var resolver *sessionResolver
	read := prefetchSources(sources, func(s sessions.CallSource) bool {
		if !since.IsZero() && s.Modified.Before(since) {
			return false
		}
		c := idx.chunks[s.Path]
		return c == nil || c.Source.Size != s.Size || !c.Source.Modified.Equal(s.Modified)
	}, readSource)
	for i, s := range sources {
		on[s.Path] = true
		if !since.IsZero() && s.Modified.Before(since) {
			continue
		}
		cached := idx.chunks[s.Path]
		c := cached.unpack()
		reused := c != nil && c.Source.Size == s.Size && c.Source.Modified.Equal(s.Modified)
		if !reused {
			if resolver == nil {
				resolver = newSessionResolver([]sessions.Call{{}})
			}
			cs := read(i)
			c = &rowChunk{Source: s, Rows: make([]packedRow, 0, len(cs))}
			for _, call := range cs {
				r := logRecord(call)
				r.SessionAccount = resolver.resolve(call)
				r.SessionOfficialLogin = resolver.officialLogin(call, r.SessionAccount)
				r.SessionProvider = call.Upstream
				row := priceRow(r, "log")
				row.codexActor = codexActor(call)
				c.add(row, call.Msg, call.To, call.Error != "")
			}
			c.Bytes -= int64(32 * len(c.Strings))
			c.dict, c.shapeIDs = nil, nil // local chunks are immutable until the source changes
			idx.chunks[s.Path] = c
		}
		next := *c
		c = &next
		c.Used = idx.tick
		if reused {
			kept := *cached
			kept.Used = idx.tick
			idx.chunks[s.Path] = &kept
		} else {
			idx.chunks[s.Path] = c.pack()
		}
		chunks = append(chunks, c)
	}
	// the other computers' days (#542), a chunk each, read again only when
	// sync brought a new copy
	var others []*rowChunk
	var renames map[string]string
	from := ""
	if !since.IsZero() { // as sharedSources reads it
		from = since.AddDate(0, 0, -2).Format(time.DateOnly)
	}
	for _, s := range days {
		on[s.path] = true
		if s.day < from {
			continue
		}
		cached := idx.chunks[s.path]
		c := cached.unpack()
		reused := c != nil && c.Source.Size == s.size && c.Source.Modified.Equal(s.mod)
		if !reused {
			d, _ := readShared(s.path)
			if renames == nil {
				renames = provider.Renamed()
			}
			c = &rowChunk{Source: sessions.CallSource{Path: s.path, Size: s.size, Modified: s.mod}, Computer: s.computer, Rows: make([]packedRow, 0, len(d.Calls))}
			for i, call := range d.Calls {
				r := call.Record
				r.Computer = ""
				if id, ok := renames[r.Provider]; ok {
					r.Provider = id
				}
				c.add(priceRow(r, call.Source), "", int64(i), false)
			}
			c.Bytes -= int64(32 * len(c.Strings))
			c.dict, c.shapeIDs = nil, nil
		}
		next := *c
		c = &next
		c.Used = idx.tick
		if reused {
			kept := *cached
			kept.Used = idx.tick
			idx.chunks[s.path] = &kept
		} else {
			idx.chunks[s.path] = c.pack()
		}
		others = append(others, c)
	}
	for path := range idx.chunks {
		if !on[path] {
			delete(idx.chunks, path)
		}
	}
	var names map[string]string
	if someShared {
		names = sharedNames()
	}
	if building != nil {
		building()
	}
	page = buildRequestBlocksWithCodex(p, f, offset, limit, now, gateways, chunks, others, names, codexIdentities)
	priced.release()
	shared.Lock()
	defer shared.Unlock()
	// A query for an older filesystem snapshot may finish after a newer one.
	if shared.root != idx.root || shared.meta != idx.meta || shared.key != idx.key || snapshot.version != version {
		return page
	}
	for path, c := range idx.chunks {
		if prev := shared.chunks[path]; prev == nil || prev.Used <= c.Used {
			shared.chunks[path] = c
		}
	}
	for path := range shared.chunks {
		if !on[path] {
			delete(shared.chunks, path)
		}
	}
	if len(shared.pages) >= 16 {
		shared.pages = map[pageKey]RequestPage{}
	}
	shared.pages[q] = page
	// The session files' chunks are bounded; the gateway's index is not
	// counted here, as it is kept whatever its size (log_index.go).
	var bytes int64
	type cachedChunk struct {
		path string
		row  *rowChunk
	}
	var kept []cachedChunk
	for path, c := range shared.chunks {
		bytes += c.Bytes
		kept = append(kept, cachedChunk{path: path, row: c})
	}
	slices.SortFunc(kept, func(a, b cachedChunk) int {
		if a.row.Used < b.row.Used {
			return -1
		}
		if a.row.Used > b.row.Used {
			return 1
		}
		return strings.Compare(a.path, b.path)
	})
	for _, c := range kept {
		if bytes <= requestCacheBytes {
			break
		}
		delete(shared.chunks, c.path)
		bytes -= c.row.Bytes
	}
	return page
}

// localRefs visits first occurrences of Claude message IDs. It stores only
// references for matches, never a full hydrated row for every call.
func visibleLocal(chunks []*rowChunk) map[rowRef]bool {
	duplicates := map[rowRef]bool{}
	seen := map[string]bool{}
	for _, c := range chunks {
		for i := range c.Rows {
			p := &c.Rows[i]
			msg := c.Strings[c.text(p, rowMsg)]
			if msg == "" {
				continue
			}
			if seen[msg] {
				duplicates[rowRef{c, i}] = true
			} else {
				seen[msg] = true
			}
		}
	}
	return duplicates
}

// matchKey narrows candidates before comparing their end times. Request IDs
// are consumed first; fallback matching still requires uniqueness both ways.
// The agent isn't in it: a session's id names the one conversation, and a
// client built on Codex (miyi-helper, say) is logged under its own name by
// the gateway while its Codex session file names Codex, so its calls were
// counted twice, once through magpie and once as not.
type matchKey struct {
	session       string
	tokens        [3]int64
	failed, hasID bool
}

// matchTokens is what a call's tokens are matched by: its input with what it
// wrote to the cache in it, its output and what it read from the cache. The
// two logs needn't split a cache write out of the input alike (#589): a
// Codex that names cache_write_input_tokens beside a gateway record that
// didn't, an older Codex that doesn't beside one that does; the sum is the
// same either way.
func matchTokens(t [6]int64) [3]int64 { return [3]int64{t[0] + t[3], t[1], t[2]} }

type matchEnd struct {
	at    time.Time
	index rowRef
}
type matchGroup struct {
	ends   []matchEnd
	counts []int
}

// agyMatchSession stands for the session of an Antigravity CLI (agy) call
// on both sides: agy sends the gateway no session header, so its gateway
// records name none, and its transcript's steps are matched to them by
// their tokens and time alone (#2097).
const agyMatchSession = "\x00agy"

// agyStepLongest is how long after an agy step began its gateway call may
// have ended: a transcript's created_at is when the step began, to the
// second, where every other session file's time is when its call ended.
const agyStepLongest = 30 * time.Minute

func matchedLocal(gateway *rowChunk, chunks []*rowChunk, skip map[rowRef]bool, since time.Time) map[rowRef]bool {
	return matchedBlocks([]*rowChunk{gateway}, chunks, skip, nil, since, true)
}

func matchedBlocks(gateways []*rowChunk, chunks []*rowChunk, skip map[rowRef]bool, responsePairs map[rowRef]rowRef, since time.Time, newestIDs bool) map[rowRef]bool {
	reserved := make(map[rowRef]bool, len(responsePairs)*2)
	matched := make(map[rowRef]bool, len(responsePairs))
	for gateway, local := range responsePairs {
		reserved[gateway], reserved[local] = true, true
		matched[local] = true
	}
	gatewaySince := since
	if !since.IsZero() {
		gatewaySince = since.Add(-24 * time.Hour)
	}
	byID := map[string][]rowRef{}
	for _, gateway := range gateways {
		for i := range gateway.Rows {
			ref := rowRef{gateway, i}
			if reserved[ref] {
				continue
			}
			p := &gateway.Rows[i]
			if (p.Flags&4 != 0 || gateway.text(p, 1) == 0 && p.Status >= 400) || p.Time.Before(gatewaySince) {
				continue
			}
			if id := gateway.Strings[gateway.text(p, 11)]; id != "" {
				byID[id] = append(byID[id], rowRef{gateway, i})
			}
		}
	}
	byRequest := map[string][]rowRef{}
	for _, c := range chunks {
		for i := range c.Rows {
			p := &c.Rows[i]
			ref := rowRef{c, i}
			if skip[ref] || reserved[ref] || p.Time.Before(since) {
				continue
			}
			if id := c.Strings[c.text(p, 11)]; len(byID[id]) > 0 {
				byRequest[id] = append(byRequest[id], ref)
			}
		}
	}
	used := map[rowRef]bool{}
	for id, refs := range byRequest {
		if newestIDs {
			slices.SortFunc(refs, func(a, b rowRef) int {
				if refNewer(a, b) {
					return -1
				}
				if refNewer(b, a) {
					return 1
				}
				return 0
			})
		}
		for j, ref := range refs {
			if j >= len(byID[id]) {
				break
			}
			matched[ref], used[byID[id][j]] = true, true
		}
	}
	groups := map[matchKey]*matchGroup{}
	for _, gateway := range gateways {
		for i := range gateway.Rows {
			ref := rowRef{gateway, i}
			if reserved[ref] {
				continue
			}
			p := &gateway.Rows[i]
			if used[ref] || (p.Flags&4 != 0 || gateway.text(p, 1) == 0 && p.Status >= 400) || p.Time.Before(gatewaySince) {
				continue
			}
			session := gateway.Strings[gateway.text(p, 14)]
			if session == "" {
				session = gateway.Strings[gateway.text(p, 13)]
			}
			if session == "" && gateway.Strings[gateway.text(p, 0)] == "agy" {
				session = agyMatchSession
			}
			if session == "" {
				continue
			}
			key := matchKey{session, matchTokens(p.Tokens), p.Status >= 400 || gateway.text(p, 9) != 0, gateway.text(p, 11) != 0}
			g := groups[key]
			if g == nil {
				g = &matchGroup{}
				groups[key] = g
			}
			g.ends = append(g.ends, matchEnd{p.Time.Add(time.Duration(p.Millis) * time.Millisecond), rowRef{gateway, i}})
		}
	}
	for _, g := range groups {
		slices.SortFunc(g.ends, func(a, b matchEnd) int { return a.at.Compare(b.at) })
		g.counts = make([]int, len(g.ends)+1)
	}
	candidates := map[rowRef]rowRef{}
	for _, c := range chunks {
		for j := range c.Rows {
			p := &c.Rows[j]
			ref := rowRef{c, j}
			if skip[ref] || reserved[ref] || matched[ref] || p.Time.Before(since) {
				continue
			}
			failed := p.Flags&16 != 0
			if p.Tokens[0]+p.Tokens[1]+p.Tokens[2]+p.Tokens[3] == 0 && !failed {
				continue
			}
			key := matchKey{session: c.Strings[c.text(p, 13)], tokens: matchTokens(p.Tokens), failed: failed}
			from, to := p.Time.Add(-2*time.Second), p.Time.Add(2*time.Second)
			if c.Strings[c.text(p, 0)] == "agy" {
				key.session, to = agyMatchSession, p.Time.Add(agyStepLongest)
			}
			count, candidate := 0, (rowRef{})
			// Without a local ID either gateway partition can match. With an ID,
			// only an unnamed gateway call can match (different IDs stay distinct).
			for _, hasID := range []bool{false, true} {
				if hasID && c.text(p, 11) != 0 {
					continue
				}
				key.hasID = hasID
				g := groups[key]
				if g == nil {
					continue
				}
				lo := sort.Search(len(g.ends), func(i int) bool { return !g.ends[i].at.Before(from) })
				hi := sort.Search(len(g.ends), func(i int) bool { return g.ends[i].at.After(to) })
				if hi > lo {
					count += hi - lo
					candidate = g.ends[lo].index
					// Range counts preserve ambiguity even for thousands of identical
					// retries, without enumerating their Cartesian product.
					g.counts[lo]++
					g.counts[hi]--
				}
			}
			if count == 1 {
				candidates[ref] = candidate
			}
		}
	}
	unique := map[rowRef]bool{}
	for _, g := range groups {
		count := 0
		for i, end := range g.ends {
			count += g.counts[i]
			if count == 1 {
				unique[end.index] = true
			}
		}
	}
	for ref, i := range candidates {
		if unique[i] {
			matched[ref] = true
		}
	}
	return matched
}
func sharesOf(m map[string]*Share) []Share {
	out := make([]Share, 0, len(m))
	for _, s := range m {
		out = append(out, *s)
	}
	slices.SortFunc(out, func(a, b Share) int {
		if a.AllTokens() != b.AllTokens() {
			return b.AllTokens() - a.AllTokens()
		}
		if a.Calls != b.Calls {
			return b.Calls - a.Calls
		}
		return strings.Compare(a.ID, b.ID)
	})
	return out
}
func buildRequestPage(p Period, f Filter, offset, limit int, gateway *rowChunk, chunks []*rowChunk) RequestPage {
	return buildRequestBlocks(p, f, offset, limit, Clock(), []*rowChunk{gateway}, chunks, nil, nil)
}

// Shared days are never matched to this computer's gateway or session calls.
// now is the moment the page is asked at: its period and its chart's hours or
// days are read from it.
func buildRequestBlocks(p Period, f Filter, offset, limit int, now time.Time, gateways, chunks, others []*rowChunk, names map[string]string) RequestPage {
	return buildRequestBlocksWithCodex(p, f, offset, limit, now, gateways, chunks, others, names, sessions.CodexIdentities())
}

func buildRequestBlocksWithCodex(p Period, f Filter, offset, limit int, now time.Time, gateways, chunks, others []*rowChunk, names map[string]string, identities map[string]sessions.CodexIdentity) RequestPage {
	gateways, chunks, others = cloneCodexChunks(gateways), cloneCodexChunks(chunks), cloneCodexChunks(others)
	duplicateSkip := foldCodexRows(chunks)
	pairs := codexChunkResponsePairs(gateways, chunks, duplicateSkip)
	for _, set := range [][]*rowChunk{gateways, chunks, others} {
		for _, chunk := range set {
			chunk.codexIdentities = identities
		}
	}
	skip := visibleLocal(chunks)
	for ref := range duplicateSkip {
		skip[ref] = true
	}
	since, until := p.Since(now), p.Until(now)
	matched := matchedBlocks(gateways, chunks, skip, pairs, since, true)
	for gateway, local := range pairs {
		actor := ""
		if local.Index < len(local.Chunk.codexActors) {
			actor = local.Chunk.codexActors[local.Index]
		}
		setCodexActor(gateway.Chunk, gateway.Index, actor)
		gateway.Chunk.setCodexCompaction(gateway.Index, local.row().Compaction)
	}
	all := append(append(slices.Clone(gateways), chunks...), others...)
	visit := func(fn func(rowRef, Row)) {
		for _, c := range all {
			for i := range c.Rows {
				pr := &c.Rows[i] // not a copy: a row is 216 bytes
				ref := rowRef{c, i}
				if pr.Time.Before(since) || after(until, pr.Time) || len(skip) > 0 && skip[ref] || len(matched) > 0 && matched[ref] {
					continue
				}
				fn(ref, c.row(i))
			}
		}
	}
	out := RequestPage{Rows: []Row{}, Agents: []string{}, Providers: []string{}, Purposes: []string{}, By: map[string][]Share{}}
	agents, providers := map[string]bool{}, map[string]bool{}
	kinds := map[string]bool{}
	callers := map[string]*Group{}
	accounts := map[[2]string]*Group{}
	computers := map[string]*Share{}
	groups := map[string]map[string]*Share{}
	chartGroups := map[string]map[string]*Share{}
	seriesGroups := map[string]map[string]*Share{}
	keys := keyer{}
	for _, d := range Dimensions {
		groups[d] = map[string]*Share{}
		chartGroups[d] = map[string]*Share{}
		seriesGroups[d] = map[string]*Share{}
	}
	// Bound the requested prefix before adding limit: external offsets can
	// reach MaxInt, and a page beyond all sources needs no heap at all.
	maxRows := 0
	for _, c := range all {
		maxRows += len(c.Rows)
	}
	take := 0
	if offset < maxRows {
		take = offset + min(limit, maxRows-offset)
	}
	selected := newestHeap{}
	chartFilter := f
	chartFilter.Day = ""
	// the two source cells' own numbers, counted without the source pick
	viaFilter := f
	viaFilter.Through = ""
	var first time.Time
	// the rows the chart and the heatmap count, kept as the first pass
	// finds them so that theirs needn't read every row of the period again
	var charted []rowRef
	visit(func(ref rowRef, r Row) {
		// looked up before they are set: most rows name what an earlier
		// one did, and a map's lookup is cheaper than its write
		if !agents[r.Agent] {
			agents[r.Agent] = true
		}
		if !kinds[r.Kind] {
			kinds[r.Kind] = true
		}
		if r.Provider != "" && !providers[r.Provider] {
			providers[r.Provider] = true
		}
		addCallerRow(callers, r)
		addAccountRow(accounts, r)
		keep := f.keepsRow(r)
		if keep {
			out.Total++
			if take > 0 && len(selected) < take {
				heap.Push(&selected, ref)
			} else if take > 0 && refNewer(ref, selected[0]) {
				selected[0] = ref
				heap.Fix(&selected, 0)
			}
			if !r.IsRejected() {
				out.Sum.addRow(r)
			}
		}
		// The two sources are counted with the Via pick cleared, as the
		// dimension groups above are: the cells are what the reader
		// switches between, so each must go on saying its own number
		// while another is picked, or one of them would read 0 and
		// there would be no way back.
		if !r.IsRejected() && viaFilter.keepsRow(r) {
			switch r.Source {
			case "log":
				out.Direct.addRow(r)
			case SourceReported:
				out.Reported.addRow(r)
			default:
				out.Through.addRow(r)
			}
		}
		if r.IsRejected() {
			return
		}
		if keep || f.Day != "" && chartFilter.keepsRow(r) {
			charted = append(charted, ref)
			for _, d := range Dimensions {
				k := keys.key(r, d)
				s := seriesGroups[d][k]
				if s == nil {
					s = &Share{ID: k}
					seriesGroups[d][k] = s
				}
				s.addRow(r)
			}
			if first.IsZero() || r.Time.Before(first) {
				first = r.Time
			}
		}
		if g := f; names != nil {
			g.Computer = ""
			g.Day = ""
			if g.keepsRow(r) {
				k := r.key("computer")
				if computers[k] == nil {
					computers[k] = &Share{ID: k}
				}
				computers[k].addRow(r)
			}
		}
		for _, d := range Dimensions {
			g := f.Without(d)
			if f.Day != "" {
				g.Day = ""
				if g.keepsRow(r) {
					k := keys.key(r, d)
					if chartGroups[d][k] == nil {
						chartGroups[d][k] = &Share{ID: k}
					}
					chartGroups[d][k].addRow(r)
				}
				g.Day = f.Day
			}
			if !g.keepsRow(r) {
				continue
			}
			k := keys.key(r, d)
			s := groups[d][k]
			if s == nil {
				s = &Share{ID: k}
				groups[d][k] = s
			}
			s.addRow(r)
		}
	})
	slices.SortFunc(selected, func(a, b rowRef) int {
		if refNewer(a, b) {
			return -1
		}
		if refNewer(b, a) {
			return 1
		}
		return 0
	})
	for _, ref := range selected[min(offset, len(selected)):] {
		out.Rows = append(out.Rows, ref.row())
	}
	for a := range agents {
		out.Agents = append(out.Agents, a)
	}
	slices.Sort(out.Agents)
	for a := range providers {
		out.Providers = append(out.Providers, a)
	}
	slices.Sort(out.Providers)
	purposes := map[string]bool{}
	for kind := range kinds {
		purposes[PurposeOf(kind)] = true
	}
	for purpose := range purposes {
		out.Purposes = append(out.Purposes, purpose)
	}
	slices.Sort(out.Purposes)
	out.CallerKeys = callerGroups(callers)
	out.Accounts = callerGroups(accounts)
	for _, d := range Dimensions {
		out.By[d] = sharesOf(groups[d])
	}
	if f.Day != "" {
		out.ChartBy = map[string][]Share{}
		for _, d := range Dimensions {
			out.ChartBy[d] = sharesOf(chartGroups[d])
		}
	}
	out.Computers, out.Names = computerShares(computers, names)
	var base []Point
	chartSince := since
	chartSince, out.Bucket, base = timeline(p, now, first)
	out.Series = make([]SeriesPoint, len(base))
	for i := range base {
		out.Series[i] = SeriesPoint{Point: base[i], By: map[string]map[string]Part{}}
		for _, d := range Dimensions {
			out.Series[i].By[d] = map[string]Part{}
		}
	}
	for _, ref := range charted {
		r := ref.row()
		t := r.Time.In(now.Location())
		if t.Before(chartSince) {
			continue
		}
		i := bucketIndex(out.Bucket, chartSince, t)
		if i < 0 || i >= len(out.Series) {
			continue
		}
		pt := &out.Series[i]
		pt.addRow(r)
		for _, d := range Dimensions {
			k := keys.key(r, d)
			part := pt.By[d][k]
			part.add(r)
			pt.By[d][k] = part
		}
	}
	if p == heatmapPeriod {
		out.Heat = heatmapOf(since, now, func(add func(Row)) {
			for _, ref := range charted {
				add(ref.row())
			}
		})
	}
	// Match LedgerSeries's top-24 selection on the filtered data, not facets.
	for _, d := range Dimensions {
		kept := map[string]bool{}
		for i, s := range sharesOf(seriesGroups[d]) {
			if i < seriesKeep {
				kept[s.ID] = true
			}
		}
		for i := range out.Series {
			for k := range out.Series[i].By[d] {
				if !kept[k] {
					delete(out.Series[i].By[d], k)
				}
			}
		}
	}
	return out
}
func pageFromLedger(p Period, f Filter, offset, limit int, all Ledgered) RequestPage {
	return pageFromLedgerAt(p, f, offset, limit, all, Clock())
}

// pageFromLedgerAt is the page of a ledger read at now, its chart over the
// period read at the same moment.
func pageFromLedgerAt(p Period, f Filter, offset, limit int, all Ledgered, now time.Time) RequestPage {
	l := all.Filtered(f)
	out := RequestPage{Sum: l.Sum, Total: len(l.Rows), Agents: l.Agents, Providers: l.Providers, Purposes: []string{}, By: map[string][]Share{}}
	// the same split the streaming path makes, so both pages of the ledger
	// agree: "log" is a call read from an agent's own session file, which
	// the gateway never saw; the rest went through it. Counted with the source
	// pick cleared, as the streaming path does, so the three cells stay
	// switchable whichever one is picked.
	viaFilter := f
	viaFilter.Through = ""
	for _, r := range all.Rows {
		if r.IsRejected() || !viaFilter.keepsRow(r) {
			continue
		}
		switch r.Source {
		case "log":
			out.Direct.addRow(r)
		case SourceReported:
			out.Reported.addRow(r)
		default:
			out.Through.addRow(r)
		}
	}
	callers, accounts := map[string]*Group{}, map[[2]string]*Group{}
	purposes := map[string]bool{}
	for i := len(all.Rows) - 1; i >= 0; i-- {
		purposes[PurposeOf(all.Rows[i].Kind)] = true
		addCallerRow(callers, all.Rows[i])
		addAccountRow(accounts, all.Rows[i])
	}
	for purpose := range purposes {
		out.Purposes = append(out.Purposes, purpose)
	}
	slices.Sort(out.Purposes)
	out.CallerKeys = callerGroups(callers)
	out.Accounts = callerGroups(accounts)
	offset = min(offset, len(l.Rows))
	out.Rows = l.Rows[offset:min(len(l.Rows), offset+limit)]
	chartRows := l.Rows
	if f.Day != "" {
		chartFilter := f
		chartFilter.Day = ""
		chartRows = all.Filtered(chartFilter).Rows
		out.ChartBy = map[string][]Share{}
	}
	out.Bucket, out.Series = ledgerSeriesAt(p, chartRows, now)
	if p == heatmapPeriod {
		out.Heat = heatmapOf(p.Since(now), now, func(add func(Row)) {
			for _, r := range chartRows {
				add(r)
			}
		})
	}
	for _, d := range Dimensions {
		g := f.Without(d)
		out.By[d] = Breakdown(all.Filtered(g).Rows, d)
		if f.Day != "" {
			g.Day = ""
			out.ChartBy[d] = Breakdown(all.Filtered(g).Rows, d)
		}
	}
	if names := sharedNames(); len(names) > 0 {
		g := f
		g.Computer = ""
		g.Day = ""
		computers := map[string]*Share{}
		for _, s := range Breakdown(all.Filtered(g).Rows, "computer") {
			computers[s.ID] = &s
		}
		out.Computers, out.Names = computerShares(computers, names)
	}
	return out
}

// computerShares are the computers' shares, this one and every other kept
// here among them though it made no call in the period; none without names.
func computerShares(m map[string]*Share, names map[string]string) ([]Share, map[string]string) {
	if names == nil {
		return nil, nil
	}
	for _, id := range append([]string{ThisComputer}, slices.Collect(maps.Keys(names))...) {
		if m[id] == nil {
			m[id] = &Share{ID: id}
		}
	}
	return sharesOf(m), names
}

// Caller choices cover the period, not just the current page or selected key.
func addCallerRow(groups map[string]*Group, r Row) {
	if r.CallerKeyID == "" || r.IsRejected() {
		return
	}
	g := groups[r.CallerKeyID]
	if g == nil {
		g = &Group{ID: r.CallerKeyID, CallerKeyID: r.CallerKeyID}
		groups[r.CallerKeyID] = g
	}
	g.CallerKeyName = r.CallerKeyName
	g.addRow(r)
}

// Account choices cover the period too: each account that answered a call,
// by provider, for the Account filter, whatever else is picked.
func addAccountRow(groups map[[2]string]*Group, r Row) {
	who := r.Account()
	if who == "" || r.IsRejected() {
		return
	}
	k := [2]string{r.Provider, who}
	g := groups[k]
	if g == nil {
		g = &Group{ID: r.Provider + "@" + who, Provider: r.Provider, Account: who}
		groups[k] = g
	}
	g.addRow(r)
}

func callerGroups[K comparable](groups map[K]*Group) []Group {
	out := make([]Group, 0, len(groups))
	for _, g := range groups {
		out = append(out, *g)
	}
	slices.SortFunc(out, func(a, b Group) int {
		if a.Tokens() != b.Tokens() {
			return b.Tokens() - a.Tokens()
		}
		if a.Calls != b.Calls {
			return b.Calls - a.Calls
		}
		return strings.Compare(a.ID, b.ID)
	})
	return out
}

// prefetchSources reads the session files a query has to parse a few at a
// time, ahead of the loop that takes them in order. The first All after
// magpie starts parses every one of them, and one by one that was a wait on
// the disk for each: Requests stayed a skeleton for 40 s over 12k Codex
// sessions. read(i) gives source i's calls; at most a window of them waits
// to be taken, so the history is never all held at once. A source that
// wasn't wanted is read when it is asked for.
func prefetchSources(sources []sessions.CallSource, want func(sessions.CallSource) bool, readSource func(sessions.CallSource) []sessions.Call) func(int) []sessions.Call {
	type slot struct {
		done  chan struct{}
		calls []sessions.Call
	}
	slots := map[int]*slot{}
	var order []int
	for i, s := range sources {
		if want(s) {
			slots[i] = &slot{done: make(chan struct{})}
			order = append(order, i)
		}
	}
	if len(order) < 2 {
		return func(i int) []sessions.Call { return readSource(sources[i]) }
	}
	workers := min(8, len(order))
	window := make(chan struct{}, 4*workers)
	next := make(chan int)
	go func() {
		for _, i := range order {
			window <- struct{}{}
			next <- i
		}
		close(next)
	}()
	for range workers {
		go func() {
			for i := range next {
				s := slots[i]
				s.calls = readSource(sources[i])
				close(s.done)
			}
		}()
	}
	return func(i int) []sessions.Call {
		s := slots[i]
		if s == nil {
			return readSource(sources[i])
		}
		<-s.done
		<-window
		cs := s.calls
		s.calls = nil
		return cs
	}
}
