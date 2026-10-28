package usage

// The request page keeps compact per-source records, not a second full ledger.
// Unchanged sources are never decoded again. Aggregation streams those records;
// only the requested page is materialized/sorted. Export can still ask for the
// complete ledger explicitly. This cache is disposable and bounded.

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"hash"
	"hash/maphash"
	"io"
	"maps"
	"math"
	"os"
	"path/filepath"
	"reflect"
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
	Masking                        int32 // ms (Record.Masking)
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

// rowCacheUnknown is the Flags bit of Record.CacheUnknown: its vendor
// counted no cache (#3464).
const rowCacheUnknown = 8192

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
	// serial names the chunk as made, for the cells a page keeps of it
	// (chunkSerial)
	serial uint64
	// mapped is the file Archive was read from, when it is of its pages
	mapped *diskMapping
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
	p := packedRow{Time: r.Time, Tokens: [6]int64{int64(r.Input), int64(r.Output), int64(r.CacheRead), int64(r.CacheWrite), int64(r.Reasoning), int64(r.CacheWrite1h)}, Millis: r.Millis, TTFT: r.TTFT, FirstText: r.FirstText, FirstReasoning: r.FirstReasoning, Sent: r.Sent, Flow: r.Flow, Own: r.Own, Masking: int32(min(r.Masking, math.MaxInt32)), Retried: r.Retried, Queued: r.Queued, Order: order, RouteID: r.RouteID, Cost: r.Cost, Status: int32(r.Status), Tries: int32(r.Tries)}
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
	if r.CacheUnknown {
		p.Flags |= rowCacheUnknown
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
	r := Row{Record: Record{RouteID: p.RouteID, Time: p.Time, Input: int(p.Tokens[0]), Output: int(p.Tokens[1]), CacheRead: int(p.Tokens[2]), CacheWrite: int(p.Tokens[3]), CacheWrite1h: int(p.Tokens[5]), Reasoning: int(p.Tokens[4]), Millis: p.Millis, TTFT: p.TTFT, FirstText: p.FirstText, FirstReasoning: p.FirstReasoning, ReasoningHidden: p.Flags&rowReasoningHidden != 0, CacheUnknown: p.Flags&rowCacheUnknown != 0, Sent: p.Sent, Flow: p.Flow, Own: p.Own, Masking: int64(p.Masking), Retried: p.Retried, Queued: p.Queued, Status: int(p.Status), Tries: int(p.Tries), HandedOn: p.Flags&rowHandedOn != 0, Rejected: p.Flags&4 != 0, SessionOfficialLogin: p.Flags&8 != 0}, Cost: p.Cost, Priced: p.Flags&1 != 0, Swapped: p.Flags&2 != 0, Routed: p.Flags&32 != 0}
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
	} else if r.Swapped && ListedAs(r.Provider, r.Model, r.Served) {
		r.Swapped = false // the name the provider's list gives the model asked for
	}
	if r.cacheNotCounted() {
		r.CacheUnknown = true // kept before its plugin said its vendor counts no cache
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

func (r rowRef) row() Row { return r.Chunk.row(r.Index) }
func refNewer(a, b rowRef) bool {
	return timedOf(a, &a.Chunk.Rows[a.Index]).newer(timedOf(b, &b.Chunk.Rows[b.Index]))
}

// timedRef is a ref with what puts it in order, newest first: its row's
// time and order. A packed chunk's refs are put in order without its rows.
type timedRef struct {
	rowRef
	at    time.Time
	order int64
}

func timedOf(ref rowRef, p *packedRow) timedRef { return timedRef{ref, p.Time, p.Order} }

func (a timedRef) newer(b timedRef) bool {
	if !a.at.Equal(b.at) {
		return a.at.After(b.at)
	}
	if (a.Chunk.Source.Path == "") != (b.Chunk.Source.Path == "") {
		return a.Chunk.Source.Path == ""
	}
	if a.Chunk.Source.Path != b.Chunk.Source.Path {
		return a.Chunk.Source.Path > b.Chunk.Source.Path
	}
	return a.order > b.order
}

type newestHeap []timedRef

func (h newestHeap) Len() int           { return len(h) }
func (h newestHeap) Less(i, j int) bool { return h[j].newer(h[i]) }
func (h newestHeap) Swap(i, j int)      { h[i], h[j] = h[j], h[i] }
func (h *newestHeap) Push(v any)        { *h = append(*h, v.(timedRef)) }
func (h *newestHeap) Pop() any          { a := *h; v := a[len(a)-1]; *h = a[:len(a)-1]; return v }

// rowsIn is how many rows c has, packed or not.
func rowsIn(c *rowChunk) int {
	if len(c.Archive) != 0 {
		return c.Count
	}
	return len(c.Rows)
}

// chunkRows reads chunks' rows one chunk at a time. A page of the whole
// history held every session file's rows decoded at once, beside the
// packed copies it keeps: ~100 MB of a start that peaked at 508 MB over
// 420k calls. A packed chunk is decoded when a pass reaches it, into a
// buffer the next one's decode reuses, and refs still name the packed
// chunk. Its strings' and shapes' slices are reused the same way.
type chunkRows struct {
	c, open *rowChunk
	// lent: the strings are of the decoded block, not a copy of them, and
	// the next chunk is decoded over them. For a pass that keeps nothing of
	// a row past the next chunk, and clones a string it keeps: the passes
	// that pair a page's calls copied every chunk's strings five times,
	// ~250 MB of a warm start's page (#3493).
	lent bool
	b    *rowBuffer
}

// rowBuffer is what a chunkRows decodes into, kept between passes
// (rowBuffers): each of a page's passes over every chunk grew its own from
// nothing, and a warm start's page and Codex's effort after it allocated
// ~30 MB a pass to read the same rows (#3493).
type rowBuffer struct {
	rows    []packedRow
	strs    []string
	shapes  [][rowTexts]uint32
	decoded []byte
	open    rowChunk
}

var rowBuffers = sync.Pool{New: func() any { return new(rowBuffer) }}

// lentRows is a chunkRows whose strings are lent (chunkRows.lent).
func lentRows() chunkRows { return chunkRows{lent: true} }

// of is c's rows to read. What the page set on c since (a Codex actor or
// compaction, the identities) is read through c.
func (r *chunkRows) of(c *rowChunk) *rowChunk {
	if len(c.Archive) == 0 {
		return c
	}
	if r.c != c {
		if r.b == nil {
			r.b = rowBuffers.Get().(*rowBuffer)
		}
		b := r.b
		var lent *[]byte
		if r.lent {
			lent = &b.decoded
		}
		open := c.unpackReusing(&b.open, b.rows, b.strs, b.shapes, lent)
		if open == nil { // never written so: pack wrote it in this process
			b.open = rowChunk{Source: c.Source, Computer: c.Computer}
			open = &b.open
		}
		if cap(open.Rows) > cap(b.rows) {
			b.rows = open.Rows[:0]
		}
		if cap(open.Strings) > cap(b.strs) {
			b.strs = open.Strings[:0]
		}
		if cap(open.Shapes) > cap(b.shapes) {
			b.shapes = open.Shapes[:0]
		}
		r.c, r.open = c, open
	}
	r.open.codexActors, r.open.codexCompactions, r.open.codexIdentities = c.codexActors, c.codexCompactions, c.codexIdentities
	return r.open
}

// release hands r's buffers on to the next pass. Nothing r opened is read
// after: a lent string kept from it is a clone, and a row a copy.
func (r *chunkRows) release() {
	if b := r.b; b != nil {
		b.open = rowChunk{}
		clear(b.strs[:cap(b.strs)]) // the strings of a block not lent are its own
		r.b, r.c, r.open = nil, nil, nil
		rowBuffers.Put(b)
	}
}

func (r *chunkRows) row(ref rowRef) Row { return r.of(ref.Chunk).row(ref.Index) }

// rowsOf reads the rows refs name, each chunk once, in the order of chunks.
func rowsOf(chunks []*rowChunk, refs []rowRef) map[rowRef]Row {
	out := make(map[rowRef]Row, len(refs))
	byChunk := map[*rowChunk][]int{}
	for _, ref := range refs {
		byChunk[ref.Chunk] = append(byChunk[ref.Chunk], ref.Index)
	}
	var rows chunkRows
	defer rows.release()
	for _, c := range chunks {
		indexes, ok := byChunk[c]
		if !ok {
			continue
		}
		delete(byChunk, c)
		open := rows.of(c)
		for _, i := range indexes {
			out[rowRef{c, i}] = open.row(i)
		}
	}
	for c, indexes := range byChunk { // a ref to a chunk not in chunks
		open := rows.of(c)
		for _, i := range indexes {
			out[rowRef{c, i}] = open.row(i)
		}
	}
	return out
}

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
	// heats are the heatmap's last pages, kept while the agents' session
	// files are all that changed (heatFresh)
	heats map[pageKey]heatPage
	// idle trims the chunks once the page goes unasked (keepWhileAsked)
	idle *time.Timer
	// parts are the cells of each chunk the last pages of a few partKeys
	// were added up from, by chunk serial: kept until what prices or
	// names a call changes (meta), and read without the lock, since a
	// part is never written once made
	parts map[partKey]map[uint64]*chunkPart
	// pairs are the last pairing of each of a few periods (pairMemo),
	// kept as parts are
	pairs map[Period]*pairMemo
	// on are the paths the last read listed; unsaved counts the chunks
	// made since the file of them was last written (toDisk), and saving
	// says a write is under way; unsavedRows counts their rows, and
	// savedRows says a write was made for them (once a run)
	on          map[string]bool
	unsaved     int
	unsavedRows int
	saving      bool
	savedRows   bool
}

// heatPage is a heatmap's page, of the gateway log's snapshot version,
// worked out at.
type heatPage struct {
	page    RequestPage
	version uint64
	at      time.Time
}

// heatFresh is how long a heatmap is kept while only the agents' session
// files change. A live agent writes its session file all the time, so the
// page's key changed for every ask, and the Requests tab's refresh worked
// out a year of calls again each time: seconds of CPU over a long history
// (#3493). A year's calendar is the same a few seconds on; a change to
// what prices or names a call, or a call through magpie, is still shown at
// once.
var heatFresh = 30 * time.Second

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

// requestCacheBytes bounds the session files' chunks kept while the
// Requests page is in use, and requestIdleBytes once it has gone unasked
// for requestIdle. Every new call is a new page key, so the open page
// is worked out again each refresh, and a chunk dropped is a shard read,
// decoded and priced again for each: at 24 MB a history of 416,000 calls
// (63 MB of chunks) read 2,679 shards again on every refresh, a third of
// its CPU (#3493). Idle, it is held to what it was before.
var (
	requestCacheBytes int64 = 128 << 20
	requestIdleBytes  int64 = 24 << 20
	requestIdle             = 2 * time.Minute
)

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
// request index's chunks; a change to key, its pages. Codex's thread
// identities are in key alone: a chunk's rows don't hold them, the page
// gives them to its rows as it is built, and every new Codex thread, a
// subagent's too, dropped every chunk, so the next page read the whole
// history again (#3493).
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
	sources, codexIdentities := sessions.CallSourcesAndCodexIdentities()
	slices.SortFunc(sources, func(a, b sessions.CallSource) int {
		if n := a.Modified.Compare(b.Modified); n != 0 {
			return n
		}
		return strings.Compare(a.Path, b.Path)
	})
	ids, _ := json.Marshal(provider.SessionIdentities(sessions.CodexDir()))
	renamed := provider.UsageRenames().Key()
	cfg := settings.Load()
	prices, _ := json.Marshal(cfg.ModelPrices)
	wires, _ := json.Marshal(cfg.ModelWires)
	meta := fmt.Sprintf("%s|%s|%s|%s|%s|%s|%s", ids, renamed, prices, wires, statKey(provider.Path()), statKey(catalog.CachePath()), clientAgentsKey())
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
	hashCodexIdentities(h, codexIdentities)
	for _, s := range sources {
		fmt.Fprintf(h, "%s:%d:%d;", s.Path, s.Size, s.Modified.UnixNano())
	}
	days, someShared := sharedSources(time.Time{})
	for _, s := range days {
		fmt.Fprintf(h, "%s:%d:%d;", s.path, s.size, s.mod.UnixNano())
	}
	return requestState{sources: sources, codexIdentities: codexIdentities, cfg: cfg, snapshot: snapshot, days: days, someShared: someShared,
		root: requestRoot(), meta: meta, key: fmt.Sprintf("%x", h.Sum(nil))}
}

// hashCodexIdentities writes ids into h in the order of their keys. They
// were json.Marshal()ed first, ~20k threads' identities encoded into a new
// buffer of several MB on every page and effort ask.
func hashCodexIdentities(h hash.Hash, ids map[string]sessions.CodexIdentity) {
	keys := make([]string, 0, len(ids))
	for k := range ids {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	b := make([]byte, 0, 32<<10)
	for _, k := range keys {
		id := ids[k]
		for _, s := range [...]string{k, id.ID, id.Role, id.ParentSession, id.RootSession} {
			b = binary.AppendUvarint(b, uint64(len(s)))
			b = append(b, s...)
		}
		if len(b) > 30<<10 {
			h.Write(b)
			b = b[:0]
		}
	}
	h.Write(b)
}

// requestRoot is the logs a request state is read from: another home's
// answer is no answer.
func requestRoot() string { return Path() + "|" + catalog.CachePath() }

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
	codexIdentities, snapshot, someShared := st.codexIdentities, st.snapshot, st.someShared
	version, key := snapshot.version, st.key
	idx := &requestCache
	idx.Lock()
	idx.keepWhileAsked()
	idx.adopt(st)
	q := pageKey{p, f, offset, limit, now.Format(time.DateOnly)}
	if page, ok := idx.pages[q]; ok {
		idx.Unlock()
		return page
	}
	if h, ok := idx.heats[q]; ok && h.version == version && time.Since(h.at) < heatFresh {
		idx.Unlock()
		return h.page
	}
	began := time.Now()
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
	// a rolling window's start moves with now, so its cells are never the
	// same twice; the heatmap is added up on its own
	var cache *partCache
	pk := partKey{p: p, f: f, since: p.Since(now), until: p.Until(now), loc: now.Location()}
	if p.Last() == 0 && p != heatmapPeriod {
		cache = &partCache{old: idx.parts[pk], next: map[uint64]*chunkPart{}, pairs: idx.pairs[p]}
	}
	shared := idx
	idx = shared.private()
	shared.Unlock()
	b := readRequestBlocks(idx, st, p, now, readSource)
	var names map[string]string
	if someShared {
		names = sharedNames()
	}
	if building != nil {
		building()
	}
	page = buildRequestBlocksIn(p, f, offset, limit, now, b.gateways, b.chunks, b.others, names, codexIdentities, cache)
	ownStrings(reflect.ValueOf(&page).Elem())
	b.priced.release()
	shared.Lock()
	defer shared.Unlock()
	if !shared.keep(idx, b.on) {
		return page
	}
	shared.noteBuilt(b)
	if cache != nil {
		if shared.parts == nil || len(shared.parts) >= partSets && shared.parts[pk] == nil {
			shared.parts = map[partKey]map[uint64]*chunkPart{}
		}
		shared.parts[pk] = cache.next
		shared.keepPairs(p, cache.pairsNext)
	}
	if len(shared.pages) >= 16 {
		shared.pages = map[pageKey]RequestPage{}
	}
	shared.pages[q] = page
	if p == heatmapPeriod {
		if shared.heats == nil || len(shared.heats) >= 16 {
			shared.heats = map[pageKey]heatPage{}
		}
		shared.heats[q] = heatPage{page: page, version: version, at: began}
	}
	shared.trim(requestCacheBytes)
	return page
}

// codexEffortOfIndex is codexModelEffort of p's Codex rows, read through
// the request index as a page is: a build after a call reads only the
// session files that changed, where reading the ledger again opened every
// Codex shard there is (#3493).
func codexEffortOfIndex(st requestState, p Period, now time.Time) map[string][]sessions.ModelEffort {
	shared := &requestCache
	shared.Lock()
	shared.keepWhileAsked()
	shared.adopt(st)
	// the page of p just before, the warm-up's of All, paired these rows:
	// a build after it paired every session file's calls again (#3493)
	memo := shared.pairs[p]
	idx := shared.private()
	shared.Unlock()
	b := readRequestBlocks(idx, st, p, now, sessions.ReadCallSource)
	sums := effortSums{}
	ps, next := pairRowsMemo(p, now, b.gateways, b.chunks, b.others, st.codexIdentities, memo)
	ps.visitActors(func(r *Row) { sums.add(r) })
	b.priced.release()
	shared.Lock()
	if shared.keep(idx, b.on) {
		shared.noteBuilt(b)
		shared.keepPairs(p, next)
		shared.trim(requestCacheBytes)
	}
	shared.Unlock()
	return sums.values()
}

// requestBlocks are what a page of a period is worked out from: the
// gateway's blocks of it, priced, the session files' chunks and the other
// computers' days, and the paths the index holds now (on).
type requestBlocks struct {
	gateways, chunks, others []*rowChunk
	on                       map[string]bool
	priced                   *blockPricer
	// built counts the session files' chunks made again, and builtRows
	// their rows
	built, builtRows int
}

// readRequestBlocks reads the blocks of p at now into idx, a private copy
// of the request index (private): a session file or another computer's
// day is read again only when it changed since its chunk was made.
func readRequestBlocks(idx *requestIndex, st requestState, p Period, now time.Time, readSource func(sessions.CallSource) []sessions.Call) requestBlocks {
	sources, cfg, snapshot, days := st.sources, st.cfg, st.snapshot, st.days
	price := Pricer()
	// Swapped's names are a few hundred pairs over thousands of files' calls,
	// and its regexps were a third of reading them again (#3493). Each
	// reader of the session files has a pricer of its own, with its memos.
	pricers := func() func(Record, string) Row {
		price := Pricer()
		swapped := map[[4]string]bool{}
		return func(r Record, source string) Row {
			sent := r.Model
			if source == "" {
				sent = provider.SentNameIn(cfg.ModelWires, r.priceID(), r.Model, r.Effort)
			}
			row := Row{Record: r, Source: source, Routed: GroupRouted(sent, r.Served)}
			if r.Served != "" {
				k := [4]string{r.priceID(), r.Model, sent, r.Served}
				s, ok := swapped[k]
				if !ok {
					s = Swapped(sent, r.Served) && !ListedAs(r.priceID(), r.Model, r.Served)
					swapped[k] = s
				}
				row.Swapped = s
			}
			if pr := price(r); pr != nil && r.Input+r.Output > 0 {
				row.Priced = true
				row.Cost = r.CostAt(*pr)
			}
			row.Agent, row.Client = reportedAs(r.Agent)
			return row
		}
	}
	priceRow := pricers()
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
	renames := provider.UsageRenames()
	priced := newBlockPricer(renames, cfg.ModelWires, price)
	for _, packed := range snapshot.blocks {
		if !gatewaySince.IsZero() && packed.Latest.Before(gatewaySince) {
			continue
		}
		if c := priced.block(packed); c != nil {
			gateways = append(gateways, c)
		}
	}
	if c := probeChunk(snapshot.probes, gatewaySince, renames, snapshot.hosts); c != nil {
		gateways = append(gateways, c) // listed, never counted (SourceProbe)
	}
	on := map[string]bool{}
	var chunks []*rowChunk
	// made once a session file is to be read again, shared by the readers:
	// it is only read after
	resolver := sync.OnceValue(func() *sessionResolver { return newSessionResolver([]sessions.Call{{}}) })
	stale := func(s sessions.CallSource) bool {
		if !since.IsZero() && s.Modified.Before(since) {
			return false
		}
		c := idx.chunks[s.Path]
		return c == nil || c.Source.Size != s.Size || !c.Source.Modified.Equal(s.Modified)
	}
	want := map[string]sessions.CallSource{}
	for _, s := range sources {
		if stale(s) {
			want[s.Path] = s
		}
	}
	var spill *archiveSpill
	if len(want) >= requestDiskLeast {
		idx.fromDisk(want)
		spill = newArchiveSpill()
	}
	built, builtRows := 0, 0
	// A file's rows are made by the reader that read it: made one after
	// another, they were most of a cold build's wait (#3493).
	makeChunk := func() func(sessions.CallSource) *rowChunk {
		priceRow := pricers()
		return func(s sessions.CallSource) *rowChunk {
			cs := readSource(s)
			c := &rowChunk{Source: s, Rows: make([]packedRow, 0, len(cs))}
			for _, call := range cs {
				r := logRecord(call)
				r.SessionAccount = resolver().resolve(call)
				r.SessionOfficialLogin = resolver().officialLogin(call, r.SessionAccount)
				r.SessionProvider = call.Upstream
				row := priceRow(r, "log")
				row.codexActor = codexActor(call)
				c.add(row, call.Msg, call.To, call.Error != "")
			}
			c.Bytes -= int64(32 * len(c.Strings))
			c.dict, c.shapeIDs = nil, nil // local chunks are immutable until the source changes
			return c.pack()
		}
	}
	read := prefetchEach(sources, stale, makeChunk)
	for i, s := range sources {
		on[s.Path] = true
		if !since.IsZero() && s.Modified.Before(since) {
			continue
		}
		// kept packed: the page reads a chunk's rows when it reaches them
		// (chunkRows), not every file's at once
		c := idx.chunks[s.Path]
		put := false
		if reused := c != nil && c.Source.Size == s.Size && c.Source.Modified.Equal(s.Modified); !reused {
			c = read(i)
			c.serial = nextChunkSerial()
			built++
			builtRows += c.Count
			put = spill.put(c, i)
		}
		kept, next := *c, *c
		kept.Used, next.Used = idx.tick, idx.tick
		idx.chunks[s.Path] = &kept
		chunks = append(chunks, &next)
		if put {
			spill.hand(&kept, &next)
		}
	}
	spill.done(func(i int) *rowChunk { return makeChunk()(sources[i]) })
	// the other computers' days (#542), a chunk each, read again only when
	// sync brought a new copy
	var others []*rowChunk
	from := ""
	if !since.IsZero() { // as sharedSources reads it
		from = since.AddDate(0, 0, -2).Format(time.DateOnly)
	}
	for _, s := range days {
		on[s.path] = true
		if s.day < from {
			continue
		}
		c := idx.chunks[s.path]
		if reused := c != nil && c.Source.Size == s.size && c.Source.Modified.Equal(s.mod); !reused {
			d, _ := readShared(s.path)
			c = &rowChunk{Source: sessions.CallSource{Path: s.path, Size: s.size, Modified: s.mod}, Computer: s.computer, Rows: make([]packedRow, 0, len(d.Calls))}
			for i, call := range d.Calls {
				r := call.Record
				r.Computer = ""
				r.renameBy(renames)
				c.add(priceRow(r, call.Source), "", int64(i), false)
			}
			c.Bytes -= int64(32 * len(c.Strings))
			c.dict, c.shapeIDs = nil, nil
			c = c.pack()
			c.serial = nextChunkSerial()
		}
		kept, next := *c, *c
		kept.Used, next.Used = idx.tick, idx.tick
		idx.chunks[s.path] = &kept
		others = append(others, &next)
	}
	for path := range idx.chunks {
		if !on[path] {
			delete(idx.chunks, path)
		}
	}
	return requestBlocks{gateways: gateways, chunks: chunks, others: others, on: on, priced: priced, built: built, builtRows: builtRows}
}

// adopt drops what idx keeps of a state other than st: every chunk when
// what prices or names a call changed (meta), the pages when anything
// did (key). idx is locked.
// keepPairs keeps memo as p's last pairing (nil: there is none to keep).
func (idx *requestIndex) keepPairs(p Period, memo *pairMemo) {
	if idx.pairs == nil || len(idx.pairs) >= partSets && idx.pairs[p] == nil {
		idx.pairs = map[Period]*pairMemo{}
	}
	if memo != nil {
		idx.pairs[p] = memo
	} else {
		delete(idx.pairs, p)
	}
}

func (idx *requestIndex) adopt(st requestState) {
	if idx.root != st.root || idx.meta != st.meta {
		idx.root, idx.meta = st.root, st.meta
		idx.chunks = map[string]*rowChunk{}
		idx.pages = nil
		idx.heats = nil
		idx.parts = nil
		idx.pairs = nil
		idx.key = ""
	}
	if idx.key != st.key {
		idx.pages = map[pageKey]RequestPage{}
		idx.key = st.key
	}
}

// private is a copy of idx's metadata and chunk map to read into without
// its lock: published chunks are immutable, and reading, decoding, pricing
// and aggregation use the copy. It counts a use of idx (tick). idx is
// locked.
func (idx *requestIndex) private() *requestIndex {
	idx.tick++
	return &requestIndex{root: idx.root, meta: idx.meta, key: idx.key, tick: idx.tick,
		chunks: maps.Clone(idx.chunks)}
}

// keep takes into idx the chunks read into read, its private copy, when
// they are of the state idx is in: a read of an older filesystem snapshot
// may finish after a newer one. on are the paths read holds. idx is
// locked.
func (idx *requestIndex) keep(read *requestIndex, on map[string]bool) bool {
	if idx.root != read.root || idx.meta != read.meta || idx.key != read.key {
		return false
	}
	for path, c := range read.chunks {
		prev := idx.chunks[path]
		if prev != nil && prev.Used > c.Used {
			continue
		}
		if prev != nil && prev.serial == c.serial && prev.pagesRank() > c.pagesRank() {
			// copied before the file of it was written (save): its archive
			// stays of the file's pages
			next := *c
			next.Archive, next.mapped = prev.Archive, prev.mapped
			c = &next
		}
		idx.chunks[path] = c
	}
	for path := range idx.chunks {
		if !on[path] {
			delete(idx.chunks, path)
		}
	}
	return true
}

// noteBuilt counts the chunks b made, and writes the file of them once
// they are requestDiskLeast or more (a first read of the history): fewer
// are a few shard reads at the next start, and not worth writing the
// whole file (~44MB for 15.6k sessions) again. Or once a run, when they
// hold requestDiskLeastRows rows: a few long sessions that ended since
// the last write (9 files, 84.6k calls, ~1s) were made again at every
// start, and the next ones that end joined them, until they were
// requestDiskLeast files. Once, as a session still at work grows with
// every call and is made again for each. idx is locked.
func (idx *requestIndex) noteBuilt(b requestBlocks) {
	idx.on = b.on
	idx.unsaved += b.built
	idx.unsavedRows += b.builtRows
	if idx.unsaved >= requestDiskLeast {
		idx.save()
	} else if !idx.savedRows && idx.unsavedRows >= requestDiskLeastRows {
		idx.savedRows = true
		idx.save()
	}
}

// requestDiskLeastRows is how many rows of chunks made since the last
// write have the file written once a run, whatever their files.
var requestDiskLeastRows = 20000

// requestDiskSaves are the writes under way, for the tests to wait on.
var requestDiskSaves sync.WaitGroup

// savingDisk, in tests, is called by a write before it writes the file.
var savingDisk func()

// save writes the file of idx's chunks behind the lock: chunks are never
// changed once made, so a copy of the map is enough. idx is locked.
func (idx *requestIndex) save() {
	if idx.saving {
		return
	}
	idx.saving = true
	idx.unsaved, idx.unsavedRows = 0, 0 // a write that fails is tried again only for new chunks
	root, meta, chunks, on := idx.root, idx.meta, maps.Clone(idx.chunks), idx.on
	requestDiskSaves.Add(1)
	go func() {
		defer requestDiskSaves.Done()
		if savingDisk != nil {
			savingDisk()
		}
		var kept map[string]*rowChunk
		if toDisk(root, meta, chunks, on) == nil {
			kept = onDisk(root, meta, chunks)
		}
		idx.Lock()
		defer idx.Unlock()
		idx.saving = false
		// a read since has copied the chunk with a use of its own: the same
		// chunk while its serial is
		for path, c := range kept {
			if cur := idx.chunks[path]; cur != nil && cur.serial == c.serial && cur.pagesRank() < 2 {
				next := *cur
				next.Archive, next.mapped = c.Archive, c.mapped
				idx.chunks[path] = &next
			}
		}
	}()
}

// keepWhileAsked holds the chunks to requestCacheBytes while the page is
// asked for, and to requestIdleBytes once it has gone unasked for
// requestIdle. idx is locked.
func (idx *requestIndex) keepWhileAsked() {
	if idx.idle != nil {
		idx.idle.Reset(requestIdle)
		return
	}
	idx.idle = time.AfterFunc(requestIdle, func() {
		idx.Lock()
		defer idx.Unlock()
		idx.trim(requestIdleBytes)
	})
}

// trim drops the chunks used longest ago until the rest are within bound.
// The session files' chunks are bounded; the gateway's index is not
// counted here, as it is kept whatever its size (log_index.go). idx is
// locked.
func (idx *requestIndex) trim(bound int64) {
	var bytes int64
	type cachedChunk struct {
		path string
		row  *rowChunk
	}
	var kept []cachedChunk
	for path, c := range idx.chunks {
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
		if bytes <= bound {
			break
		}
		delete(idx.chunks, c.path)
		bytes -= c.row.Bytes
	}
}

// localRefs visits first occurrences of Claude message IDs. It stores only
// references for matches, never a full hydrated row for every call.
func visibleLocal(chunks []*rowChunk) map[rowRef]bool {
	duplicates := map[rowRef]bool{}
	// a message id seen is kept as two 64-bit hashes of it, not a copy of
	// the decoded chunk's string: a long history's ~400k ids held ~7MB of
	// copies at the start's peak, and a key of these is a string header's
	// size. Two ids alike in both are ~1e-28 likely.
	seen := map[[2]uint64]struct{}{}
	rows := lentRows()
	defer rows.release()
	for _, c := range chunks {
		open := rows.of(c)
		for i := range open.Rows {
			p := &open.Rows[i]
			msg := open.Strings[open.text(p, rowMsg)]
			if msg == "" {
				continue
			}
			k := [2]uint64{maphash.String(msgSeeds[0], msg), maphash.String(msgSeeds[1], msg)}
			if _, ok := seen[k]; ok {
				duplicates[rowRef{c, i}] = true
			} else {
				seen[k] = struct{}{}
			}
		}
	}
	return duplicates
}

var msgSeeds = [2]maphash.Seed{maphash.MakeSeed(), maphash.MakeSeed()}

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
	byRequest := map[string][]timedRef{}
	rows := lentRows() // a kept request id is cloned; a key is looked up
	defer rows.release()
	for _, c := range chunks {
		open := rows.of(c)
		for i := range open.Rows {
			p := &open.Rows[i]
			ref := rowRef{c, i}
			if skip[ref] || reserved[ref] || p.Time.Before(since) {
				continue
			}
			if id := open.Strings[open.text(p, 11)]; len(byID[id]) > 0 {
				if _, ok := byRequest[id]; !ok {
					id = strings.Clone(id)
				}
				byRequest[id] = append(byRequest[id], timedOf(ref, p))
			}
		}
	}
	used := map[rowRef]bool{}
	for id, refs := range byRequest {
		if newestIDs {
			slices.SortFunc(refs, func(a, b timedRef) int {
				if a.newer(b) {
					return -1
				}
				if b.newer(a) {
					return 1
				}
				return 0
			})
		}
		for j, ref := range refs {
			if j >= len(byID[id]) {
				break
			}
			matched[ref.rowRef], used[byID[id][j]] = true, true
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
	for _, chunk := range chunks {
		c := rows.of(chunk)
		for j := range c.Rows {
			p := &c.Rows[j]
			ref := rowRef{chunk, j}
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

// buildRequestBlocksWithCodex's page owns its strings. A packed chunk is
// read through one block of strings at a time (chunkRows), and a row, an
// agent's name or a group kept by the page from it would hold that whole
// block: ~9 MB after the first page, kept by the request cache.
func buildRequestBlocksWithCodex(p Period, f Filter, offset, limit int, now time.Time, gateways, chunks, others []*rowChunk, names map[string]string, identities map[string]sessions.CodexIdentity) RequestPage {
	out := buildRequestBlocksOf(p, f, offset, limit, now, gateways, chunks, others, names, identities)
	ownStrings(reflect.ValueOf(&out).Elem())
	return out
}

// ownStrings copies every string v holds that can be set, map keys too.
func ownStrings(v reflect.Value) {
	switch v.Kind() {
	case reflect.String:
		if v.CanSet() && v.Len() > 0 {
			v.SetString(strings.Clone(v.String()))
		}
	case reflect.Pointer:
		if !v.IsNil() {
			ownStrings(v.Elem())
		}
	case reflect.Struct:
		for i := range v.NumField() {
			if f := v.Field(i); f.CanSet() {
				ownStrings(f)
			}
		}
	case reflect.Slice, reflect.Array:
		for i := range v.Len() {
			ownStrings(v.Index(i))
		}
	case reflect.Map:
		if v.IsNil() || !v.CanSet() {
			return
		}
		m := reflect.MakeMapWithSize(v.Type(), v.Len())
		for it := v.MapRange(); it.Next(); {
			k, e := reflect.New(v.Type().Key()).Elem(), reflect.New(v.Type().Elem()).Elem()
			k.Set(it.Key())
			e.Set(it.Value())
			ownStrings(k)
			ownStrings(e)
			m.SetMapIndex(k, e)
		}
		v.Set(m)
	}
}

// pairedRows are the page's rows of p: each gateway call told the Codex
// thread of the session file's call it is paired with (pairs), and the
// session files' calls a gateway call matched, or another file's copy,
// left out (matched, skip). visit hands each row on in all's order. The
// chunks are copies, so the cached ones are left as they are.
type pairedRows struct {
	all           []*rowChunk
	skip, matched map[rowRef]bool
	pairs         map[rowRef]rowRef
	compactions   map[rowRef]*bool
	since, until  time.Time
}

// pairing, in tests, is told each time pairRows pairs chunks' rows.
var pairing func()

func pairRows(p Period, now time.Time, gateways, chunks, others []*rowChunk, identities map[string]sessions.CodexIdentity) *pairedRows {
	if pairing != nil {
		pairing()
	}
	gateways, chunks, others = cloneCodexChunks(gateways), cloneCodexChunks(chunks), cloneCodexChunks(others)
	duplicateSkip := foldCodexRows(chunks)
	pairs, compactions := codexChunkResponsePairs(gateways, chunks, duplicateSkip)
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
		gateway.Chunk.setCodexCompaction(gateway.Index, compactions[local])
	}
	all := append(append(slices.Clone(gateways), chunks...), others...)
	return &pairedRows{all: all, skip: skip, matched: matched, pairs: pairs, compactions: compactions, since: since, until: until}
}

// visit calls fn with each row of the period that no other one stands for.
func (ps *pairedRows) visit(fn func(timedRef, *Row)) {
	var rows chunkRows
	defer rows.release()
	// one row for the visit, written over for each: fn is not known
	// here, so a row of each would be an allocation of its own, and fn
	// keeps none of them
	var r Row
	for _, c := range ps.all {
		ps.visitChunk(&rows, c, &r, fn)
	}
}

// visitActors is visit of the rows of the chunks a Codex thread is known
// for some row of (codexActors), the only rows Codex's effort adds up: it
// decoded every session file's chunk, ~90 MB of what eight Sessions polls
// allocated, for the few that are Codex's. The rows' strings are lent
// (lentRows), so fn clones what it keeps.
func (ps *pairedRows) visitActors(fn func(*Row)) {
	rows := lentRows()
	defer rows.release()
	var r Row
	for _, c := range ps.all {
		if slices.ContainsFunc(c.codexActors, func(a string) bool { return a != "" }) {
			if visitingActors != nil {
				visitingActors(c)
			}
			ps.visitChunk(&rows, c, &r, func(_ timedRef, r *Row) { fn(r) })
		}
	}
}

// visitingActors, in tests, is told each chunk visitActors reads.
var visitingActors func(*rowChunk)

// visitChunk is visit of c's rows alone, read through rows into r.
func (ps *pairedRows) visitChunk(rows *chunkRows, c *rowChunk, r *Row, fn func(timedRef, *Row)) {
	open := rows.of(c)
	for i := range open.Rows {
		pr := &open.Rows[i] // not a copy: a row is 216 bytes
		ref := rowRef{c, i}
		if pr.Time.Before(ps.since) || after(ps.until, pr.Time) || len(ps.skip) > 0 && ps.skip[ref] || len(ps.matched) > 0 && ps.matched[ref] {
			continue
		}
		*r = open.row(i)
		fn(timedOf(ref, pr), r)
	}
}

func buildRequestBlocksOf(p Period, f Filter, offset, limit int, now time.Time, gateways, chunks, others []*rowChunk, names map[string]string, identities map[string]sessions.CodexIdentity) RequestPage {
	return buildRequestBlocksIn(p, f, offset, limit, now, gateways, chunks, others, names, identities, nil)
}

// buildRequestBlocksIn is the page added up from each chunk's cells,
// cache's for the chunks it has them for (nil: every chunk's read).
func buildRequestBlocksIn(p Period, f Filter, offset, limit int, now time.Time, gateways, chunks, others []*rowChunk, names map[string]string, identities map[string]sessions.CodexIdentity, cache *partCache) RequestPage {
	since := p.Since(now)
	var ps *pairedRows
	if cache != nil {
		ps, cache.pairsNext = pairRowsMemo(p, now, gateways, chunks, others, identities, cache.pairs)
	} else {
		ps = pairRows(p, now, gateways, chunks, others, identities)
	}
	out := RequestPage{Rows: []Row{}, Agents: []string{}, Providers: []string{}, Purposes: []string{}, By: map[string][]Share{}}
	chartFilter := f
	chartFilter.Day = ""
	// The heatmap's page is only its days, all HeatmapOf reads: over its
	// year the groups, shares and series below took seconds a call (#3493).
	if p == heatmapPeriod {
		out.Heat = heatmapOf(since, now, func(add func(Row)) {
			ps.visit(func(_ timedRef, r *Row) {
				if chartFilter.keepsRow(r) {
					add(*r)
				}
			})
		})
		return out
	}
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
	for _, c := range ps.all {
		maxRows += rowsIn(c)
	}
	take := 0
	if offset < maxRows {
		take = offset + min(limit, maxRows-offset)
	}
	// a kept cell says which computer its rows are of whether this page
	// counts computers or not: the next one of its partKey may
	pass := newCellPass(p, f, now, names != nil || cache != nil)
	parts := ps.partsOf(pass, cache, identities)
	var first time.Time
	// Each dimension's groups are counted with its own pick cleared, and
	// the computers' with the computer's and day's (cellPass): a cell's
	// bits say which of them its rows are in, and its sums (d), made once,
	// are added to each (#3493).
	var r Row
	add := func(m map[string]*Share, k string, d *Totals) {
		s := m[k]
		if s == nil {
			s = &Share{ID: k}
			m[k] = s
		}
		s.plus(d)
	}
	for _, part := range parts {
		if !part.first.IsZero() && (first.IsZero() || part.first.Before(first)) {
			first = part.first
		}
		for i := range part.cells {
			cell := &part.cells[i]
			agent, kind, prov := cell.agent.Value(), cell.kind.Value(), cell.provider.Value()
			// looked up before they are set: most cells name what an
			// earlier one did, and a map's lookup is cheaper than its write
			if !agents[agent] {
				agents[agent] = true
			}
			if !kinds[kind] {
				kinds[kind] = true
			}
			if prov != "" && !providers[prov] {
				providers[prov] = true
			}
			keep := cell.bits&cellKeep != 0
			if keep {
				out.Total += cell.n
			}
			// a call turned away, and a request magpie sent on its own
			// (SourceProbe), are listed and counted in no total
			if cell.bits&cellUncounted != 0 {
				continue
			}
			d := &cell.d
			if keep {
				out.Sum.plus(d)
			}
			if id := cell.callerID.Value(); id != "" {
				g := callers[id]
				if g == nil {
					g = &Group{ID: id, CallerKeyID: id}
					callers[id] = g
				}
				g.CallerKeyName = cell.callerName.Value()
				g.plus(d)
			}
			if who := cell.account.Value(); who != "" {
				k := [2]string{prov, who}
				g := accounts[k]
				if g == nil {
					g = &Group{ID: prov + "@" + who, Provider: prov, Account: who}
					accounts[k] = g
				}
				g.plus(d)
			}
			// The two sources are counted with the Via pick cleared, as the
			// dimension groups are: the cells are what the reader switches
			// between, so each must go on saying its own number while
			// another is picked, or one of them would read 0 and there
			// would be no way back.
			if cell.bits&cellVia != 0 {
				switch cell.source {
				case cellDirect:
					out.Direct.plus(d)
				case cellReported:
					out.Reported.plus(d)
				default:
					out.Through.plus(d)
				}
			}
			cell.row(&r)
			if cell.bits&cellCharted != 0 {
				for _, dim := range Dimensions {
					add(seriesGroups[dim], keys.key(&r, dim), d)
				}
			}
			if cell.bits&cellComputer != 0 {
				add(computers, r.key("computer"), d)
			}
			for j, dim := range Dimensions {
				if cell.bits&(cellChart<<j) != 0 {
					add(chartGroups[dim], keys.key(&r, dim), d)
				}
				if cell.bits&(cellGroup<<j) != 0 {
					add(groups[dim], keys.key(&r, dim), d)
				}
			}
		}
	}
	selected := ps.newest(f, parts, take)
	slices.SortFunc(selected, func(a, b timedRef) int {
		if a.newer(b) {
			return -1
		}
		if b.newer(a) {
			return 1
		}
		return 0
	})
	var page []rowRef
	for _, ref := range selected[min(offset, len(selected)):] {
		page = append(page, ref.rowRef)
	}
	read := rowsOf(ps.all, page)
	for _, ref := range page {
		out.Rows = append(out.Rows, read[ref])
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
	var chartSince time.Time
	chartSince, out.Bucket, base = timeline(p, now, first)
	out.Series = make([]SeriesPoint, len(base))
	for i := range base {
		out.Series[i] = SeriesPoint{Point: base[i], By: map[string]map[string]Part{}}
		for _, d := range Dimensions {
			out.Series[i].By[d] = map[string]Part{}
		}
	}
	// a cell's bucket is its hour of the period, or its day: the chart of
	// days or weeks starts at the oldest charted call's, known only now
	sinceDay := dayNumber(chartSince)
	for _, part := range parts {
		for j := range part.cells {
			cell := &part.cells[j]
			if cell.bits&cellCharted == 0 || cell.bits&cellUncounted != 0 {
				continue
			}
			i := int(cell.bucket)
			if !pass.hours {
				i = int(cell.bucket - sinceDay)
				if i >= 0 && out.Bucket == "week" {
					i /= 7
				}
			}
			if i < 0 || i >= len(out.Series) {
				continue
			}
			pt := &out.Series[i]
			d := &cell.d
			pt.plus(d)
			cell.row(&r)
			for _, dim := range Dimensions {
				k := keys.key(&r, dim)
				part := pt.By[dim][k]
				part.plus(d)
				pt.By[dim][k] = part
			}
		}
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
		if r.IsRejected() || !viaFilter.keepsRow(&r) {
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
		var d Totals
		d.addRow(all.Rows[i])
		addCallerRow(callers, &all.Rows[i], &d)
		addAccountRow(accounts, &all.Rows[i], &d)
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
func addCallerRow(groups map[string]*Group, r *Row, d *Totals) {
	if r.CallerKeyID == "" || r.IsRejected() {
		return
	}
	g := groups[r.CallerKeyID]
	if g == nil {
		g = &Group{ID: r.CallerKeyID, CallerKeyID: r.CallerKeyID}
		groups[r.CallerKeyID] = g
	}
	g.CallerKeyName = r.CallerKeyName
	g.plus(d)
}

// Account choices cover the period too: each account that answered a call,
// by provider, for the Account filter, whatever else is picked.
func addAccountRow(groups map[[2]string]*Group, r *Row, d *Totals) {
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
	g.plus(d)
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

// prefetchEach reads the session files a query has to parse a few at a
// time, ahead of the loop that takes them in order. The first All after
// magpie starts parses every one of them, and one by one that was a wait on
// the disk for each: Requests stayed a skeleton for 40 s over 12k Codex
// sessions. read(i) gives what a make from newMake made of source i; at
// most a window of them waits to be taken, so the history is never all
// held at once. A source that wasn't wanted is read when it is asked for.
// Each reader, and the loop for a source not wanted, has a make of its
// own, so what a make keeps between sources needs no lock.
func prefetchEach[T any](sources []sessions.CallSource, want func(sessions.CallSource) bool, newMake func() func(sessions.CallSource) T) func(int) T {
	type slot struct {
		done chan struct{}
		v    T
	}
	slots := map[int]*slot{}
	var order []int
	for i, s := range sources {
		if want(s) {
			slots[i] = &slot{done: make(chan struct{})}
			order = append(order, i)
		}
	}
	var own func(sessions.CallSource) T
	alone := func(i int) T {
		if own == nil {
			own = newMake()
		}
		return own(sources[i])
	}
	if len(order) < 2 {
		return alone
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
			build := newMake()
			for i := range next {
				s := slots[i]
				s.v = build(sources[i])
				close(s.done)
			}
		}()
	}
	return func(i int) T {
		s := slots[i]
		if s == nil {
			return alone(i)
		}
		<-s.done
		<-window
		v := s.v
		var zero T
		s.v = zero
		return v
	}
}
