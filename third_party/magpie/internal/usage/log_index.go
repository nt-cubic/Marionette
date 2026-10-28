package usage

// The JSONL file remains the ledger. This disposable index keeps immutable,
// compressed blocks of its metadata; appending copies at most the last block.
// Readers decode only blocks which can overlap their time window.
import (
	"bufio"
	"crypto/sha256"
	"encoding"
	"encoding/json"
	"fmt"
	"hash"
	"io"
	"maps"
	"os"
	"slices"
	"strings"
	"sync"
	"time"
)

const (
	logBlockRows   = 512
	logStampSettle = time.Second
	// logFineSettle is logStampSettle for a change time with a fraction of
	// a second, from a clock that ticks in milliseconds: APFS's in
	// nanoseconds, Linux's coarse clock in 1 to 10 ms, NTFS's in ~15.6 ms.
	logFineSettle = 100 * time.Millisecond
)

// The blocks are always kept, however long the history: they are the index
// every reader shares, about 80 bytes a call compressed. Dropping them over a
// size budget (#563) made each read parse the whole file again, 6 s of CPU
// for 750k calls, on every refresh and filter of the Requests page.

type logSnapshot struct {
	bytes   int64
	settled bool
	path    string
	info    os.FileInfo
	off     int64
	hash    string // SHA-256 of the log's first off bytes, the ones parsed
	digest  []byte // the hash's running state, which the next parse continues
	version uint64
	blocks  []*rowChunk
	first   time.Time
	// firstOwn is the first call magpie carried itself, leaving out what
	// apps reported (#1874): a backfill reaching back doesn't make the log
	// reach back to a subscription window's start.
	firstOwn     time.Time
	keyProviders map[string]bool
	// probes are the requests magpie sent on its own (SourceProbe), kept
	// out of blocks: every reader of those counts what it reads, and the
	// request log alone lists these (queryPage)
	probes []probeRow
	// hosts are the providers by the host their calls went to, as the log
	// recorded them: "" for a host two providers were on. A probe made
	// while a provider was being added, under its host, is that
	// provider's after it is deleted too (probeChunk; cybercaius).
	hosts map[string]string
}

var logIndex struct {
	sync.Mutex
	snapshot *logSnapshot
	version  uint64
	reads    int // how many times the file was opened to be parsed, for the tests
}

var logIndexNow = time.Now
var logRecordHash = recordHash

// File writes must never wait for a historical index rebuild.
var logAppends struct {
	sync.Mutex
	path       string
	base, last os.FileInfo
}

// sawHost notes the provider r's call went to by its host. A probe made
// before its provider had an id names the host for both, and says nothing.
func (s *logSnapshot) sawHost(r Record) {
	h, _, _ := strings.Cut(r.Host, " as ") // a subscription's "host as user"
	h = strings.ToLower(strings.TrimSpace(h))
	if h == "" || r.Provider == "" || strings.EqualFold(r.Provider, h) || r.IsRejected() {
		return
	}
	if s.hosts == nil {
		s.hosts = map[string]string{}
	}
	if to, ok := s.hosts[h]; !ok {
		s.hosts[h] = r.Provider
	} else if to != r.Provider {
		s.hosts[h] = ""
	}
}

func readLogSnapshot() *logSnapshot {
	// the clients the user counts as agents, read once for the pass the
	// snapshot is read for (#1958)
	readClientAgents()
	logIndex.Lock()
	defer logIndex.Unlock()
	path := Path()
	start := logIndexNow()
	info, err := statLogFile(path)
	settled := settledLogStamp(info, start)
	old := logIndex.snapshot
	unchanged := old != nil && old.path == path && (err != nil && old.info == nil || err == nil && sameLogInfo(old.info, info))
	if unchanged && info != nil {
		if logChangeStamp(info) == "" || !old.settled {
			unchanged = old.hash != "" && logRecordHash(path, old.off) == old.hash
			if unchanged && settled {
				promoted := *old
				promoted.info, promoted.settled = info, true
				old = &promoted
				logIndex.snapshot = old
			}
		} else {
			unchanged = old.settled && settled
		}
	}
	if unchanged {
		return old
	}
	if !unchanged {
		logIndex.version++
	}

	next := &logSnapshot{path: path, info: info, settled: settled, version: logIndex.version, keyProviders: map[string]bool{}, hosts: map[string]string{}}
	if err != nil {
		logIndex.snapshot = next
		return next
	}
	logAppends.Lock()
	notePath, noteBase, noteLast := logAppends.path, logAppends.base, logAppends.last
	logAppends.Unlock()
	trusted := old != nil && old.settled && logChangeStamp(info) != "" && notePath == path && sameLogInfo(oldInfo(old), noteBase) && sameLogInfo(info, noteLast)
	continued := old != nil && old.path == path && old.info != nil && sameLogFile(old.info, info) && info.Size() > old.info.Size() && (trusted || old.hash != "" && logRecordHash(path, old.off) == old.hash)
	// The parse is the fingerprint: it hashes the lines it reads, continuing
	// the old snapshot's hash when it continues its blocks.
	sum := sha256.New()
	if continued && resumeLogHash(sum, old.digest) {
		next.off, next.first, next.firstOwn = old.off, old.first, old.firstOwn
		next.blocks = slices.Clone(old.blocks)
		next.keyProviders = maps.Clone(old.keyProviders)
		next.probes = slices.Clip(old.probes) // appended to, never written in
		next.hosts = maps.Clone(old.hosts)
	}
	logIndex.reads++
	f, err := os.Open(path)
	if err != nil {
		return next
	} // never publish an incomplete read as current
	defer f.Close()
	if _, err = f.Seek(next.off, 0); err != nil {
		return next
	}
	var tail *rowChunk
	routes := routeRows{}
	if n := len(next.blocks); n > 0 && next.blocks[n-1].Count < logBlockRows {
		tail = next.blocks[n-1].writable()
		next.blocks = next.blocks[:n-1]
		for i := range tail.Rows {
			routes.seen(tail, i)
		}
	}
	rd := bufio.NewReaderSize(io.LimitReader(f, info.Size()-next.off), 64<<10)
	for next.off < info.Size() {
		b, e := rd.ReadBytes('\n')
		if e != nil {
			break
		} // a partial final line is read again after it completes
		next.off += int64(len(b))
		sum.Write(b)
		var r Record
		if json.Unmarshal(b, &r) != nil {
			continue
		}
		next.sawHost(r)
		if r.IsProbe() {
			if !quietProbe(r) { // a model list answered is no row (ferock)
				next.probes = append(next.probes, probeRow{r, next.off})
			}
			continue
		}
		if tail == nil {
			tail = &rowChunk{}
		}
		routes.handedOn(tail, &r)
		tail.add(Row{Record: r, Source: r.Source}, "", next.off, false)
		routes.seen(tail, len(tail.Rows)-1)
		if !r.IsRejected() && (next.first.IsZero() || r.Time.Before(next.first)) {
			next.first = r.Time
		}
		if !r.IsRejected() && !r.IsReported() && (next.firstOwn.IsZero() || r.Time.Before(next.firstOwn)) {
			next.firstOwn = r.Time
		}
		if r.ProviderKeyID != "" {
			next.keyProviders[r.Provider] = true
		}
		if len(tail.Rows) == logBlockRows {
			next.blocks = append(next.blocks, tail.freeze())
			tail = nil
			clear(routes)
		}
	}
	if tail != nil {
		next.blocks = append(next.blocks, tail.freeze())
	}
	next.hash, next.digest = fmt.Sprintf("%x", sum.Sum(nil)), marshalLogHash(sum)
	// A rewrite during the read must not give old blocks the new content's hash.
	if (!trusted || !settled) && next.hash != logRecordHash(path, next.off) {
		next.hash, next.digest = "", nil
		next.settled = false
	}
	logAppends.Lock()
	// Preserve writes which arrived while this immutable snapshot was built.
	if logAppends.path == path && (sameLogInfo(logAppends.last, info) || sameLogInfo(logAppends.base, info) || notePath == path && sameLogInfo(noteLast, info) && sameLogInfo(noteBase, logAppends.base)) {
		logAppends.base = info
	} else if logAppends.last == nil {
		logAppends.path, logAppends.base, logAppends.last = path, info, info
	}
	logAppends.Unlock()
	for _, c := range next.blocks {
		next.bytes += c.Bytes
	}
	logIndex.snapshot = next
	return next
}

// routeRows are the rows of the block being written, by their route: where
// each route's last row is.
type routeRows map[int64]int

func (o routeRows) seen(c *rowChunk, i int) {
	if id := c.Rows[i].RouteID; id != 0 {
		o[id] = i
	}
}

// handedOn tells, of a log written before Record.HandedOn was, the row of
// a try that failed and handed its request on: a failed row of r's route
// before r, whose outcome r is. r then ends a request with a try before
// it. Only rows in the same block are told, which is where the gateway's
// rows of one request land but for one in 512 at a block's edge.
func (o routeRows) handedOn(c *rowChunk, r *Record) {
	if r.RouteID == 0 {
		return
	}
	j, ok := o[r.RouteID]
	if !ok {
		return
	}
	p := &c.Rows[j]
	if p.Flags&rowHandedOn != 0 || p.Flags&4 != 0 || !(p.Status >= 400 || c.text(p, rowError) != 0) {
		return
	}
	p.Flags |= rowHandedOn
	r.Tries = max(r.Tries, 2)
}

func (s *logSnapshot) visit(since time.Time, fn func(Record)) {
	for _, packed := range s.blocks {
		if !since.IsZero() && packed.Latest.Before(since) {
			continue
		}
		c := packed.unpack()
		if c == nil {
			continue
		}
		for i := range c.Rows {
			p := &c.Rows[i]
			if !p.Time.Before(since) {
				fn(c.row(i).Record)
			}
		}
	}
}

func (c *rowChunk) writable() *rowChunk {
	d := c.unpack()
	if d == nil {
		return &rowChunk{}
	}
	next := *d
	next.Rows, next.Strings, next.Shapes = slices.Clone(d.Rows), slices.Clone(d.Strings), slices.Clone(d.Shapes)
	next.shapeIDs = nil
	next.dict = make(map[string]uint32, len(next.Strings))
	for i, s := range next.Strings {
		next.dict[s] = uint32(i)
	}
	next.Bytes += int64(32 * len(next.Strings))
	return &next
}

func (c *rowChunk) freeze() *rowChunk {
	if c.dict != nil {
		c.dict = nil
		c.Bytes -= int64(32 * len(c.Strings))
	}
	c.shapeIDs = nil
	c = c.pack()
	c.serial = nextChunkSerial()
	return c
}

func resumeLogHash(h hash.Hash, state []byte) bool {
	u, ok := h.(encoding.BinaryUnmarshaler)
	if !ok || state == nil || u.UnmarshalBinary(state) != nil {
		h.Reset()
		return false
	}
	return true
}

func marshalLogHash(h hash.Hash) []byte {
	m, ok := h.(encoding.BinaryMarshaler)
	if !ok {
		return nil
	}
	state, err := m.MarshalBinary()
	if err != nil {
		return nil
	}
	return state
}

func oldInfo(s *logSnapshot) os.FileInfo {
	if s == nil {
		return nil
	}
	return s.info
}

// A read in the change stamp's clock tick cannot prove that a later equal
// stamp still names the same bytes. Decide before reading, not at reuse time.
//
// A stamp in whole seconds (HFS+, FAT, a share that keeps no fraction) may
// share its second with a write after the read. One with a fraction comes
// from a clock that ticks in milliseconds, past which a write has a later
// stamp, so it settles in logFineSettle. Under steady traffic each read
// came within a second of the gateway's last append, so each one hashed
// the whole log again: 1.8 times the log for each append and read, so a
// 100 MB history cost ~180 MB of SHA-256 for every request (#3394).
func settledLogStamp(info os.FileInfo, start time.Time) bool {
	c := logChangeTime(info)
	settle := logStampSettle
	if c.Nanosecond() != 0 {
		settle = logFineSettle
	}
	return !c.IsZero() && c.Before(start.Add(-settle))
}

func sameLogInfo(a, b os.FileInfo) bool {
	return a != nil && b != nil && sameLogFile(a, b) && a.Size() == b.Size() && a.ModTime().Equal(b.ModTime()) && logChangeStamp(a) == logChangeStamp(b)
}

// Append records the exact file states around its successful write. Contiguous
// writes by this process need no historical prefix scan; external edits still
// go through the fingerprint/rebuild path.
func noteLogAppend(path string, before, after os.FileInfo) {
	logAppends.Lock()
	defer logAppends.Unlock()
	if logAppends.path != path || !sameLogInfo(before, logAppends.last) {
		logAppends.path, logAppends.base = path, before
	}
	logAppends.last = after
}
