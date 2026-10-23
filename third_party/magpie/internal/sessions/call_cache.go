package sessions

// Request metadata uses checksummed append-only frames. A growing session saves
// only new/changed calls and its parser continuation. Corrupt shards rebuild
// from source. Sessions summaries never load these files.

import (
	"bytes"
	"encoding/binary"
	"encoding/gob"
	"encoding/json"
	"hash/crc32"
	"hash/fnv"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
)

// v3 persists Codex cumulative counters explicitly; v2 shards can contain
// inflated deltas after a reload, so rebuild their derived rows from source.
// v4: a Codex call's input no longer holds what it wrote to the cache (#589).
// v5: an OpenCode call keeps the effort its prompt asked for (#680).
// v6: Codex response records and compact disk-only parser state.
// v7: reconcile recent Claude message revisions.
// v8: a Claude Code call's 1-hour cache writes.
// v9: a dsh call carries the provider it named and the model that answered,
// and a call through magpie's own gateway is left out of dsh's (the gateway
// logs it itself).
// v10: the calls ZCode's and WorkBuddy's session files hold are read too, and
// WorkBuddy's that went through the gateway are left out of them as a dsh
// one is.
// v12: a ZCode call's input leaves out the cache, and its output the
// reasoning counted twice, as its session's summary does (#1947).
// v13: a call after a compaction carries how long the compaction took
// (Compact): Claude Code's compact_boundary durationMs, Codex's compaction
// call's own time.
// v14: Codex calls keep the thread that made the request.
// v15: Codex calls keep the compaction marker independently of its duration.
// What each version names is what a cache of the one before it cannot serve:
// the version is part of the directory's name, so a cache written by an older
// one is never read (pruneCalls).
const callCacheVersion = "calls-v15"
const maxKeptCalls = 131072
const maxKeptFiles = 64

var callsMu sync.Mutex
var callIO [64]sync.Mutex
var callGeneration uint64
var callCache = map[string]*callFile{}
var callCounts = map[string]int{} // small allocation hints, never call records
var callOrder []string
var callRoot string

func callCacheDir() string             { return filepath.Join(filepath.Dir(CachePath()), callCacheVersion) }
func callCachePath(path string) string { return filepath.Join(callCacheDir(), hashHead(path)+".gob") }

func resetCalls() {
	callsMu.Lock()
	defer callsMu.Unlock()
	callCache, callOrder = map[string]*callFile{}, nil
	callContinuations, callContinuationOrder = map[string]callContinuation{}, nil
	callCounts = map[string]int{}
	callRoot = ""
	callGeneration++
	resetOCCalls()
	// ZCode's listing and WorkBuddy's gateway ids name another HOME's files
	// and models.json: a read after a Reset() would otherwise hand back
	// sources that are not here. Both are stamped, so the stale stamp alone
	// would miss; clearing them makes a new HOME read its own
	zcCallList.Lock()
	zcCallList.stamp, zcCallList.files = "", nil
	zcCallList.Unlock()
	wbGateway.Lock()
	wbGateway.byPath = nil
	wbGateway.Unlock()
}

func pruneCalls(files []file) {
	root := callCacheDir()
	callsMu.Lock()
	moved := callRoot != root
	if moved {
		callGeneration++
		callCache, callOrder, callRoot = map[string]*callFile{}, nil, root
		callCounts = map[string]int{}
	}
	on := make(map[string]bool, len(files))
	shards := make(map[string]bool, len(files))
	for _, f := range files {
		on[f.path] = true
		shards[hashHead(f.path)+".gob"] = true
	}
	order := callOrder[:0]
	for _, path := range callOrder {
		if on[path] {
			order = append(order, path)
		} else {
			delete(callCache, path)
		}
	}
	callOrder = order
	for path := range callCounts {
		if !on[path] {
			delete(callCounts, path)
		}
	}
	order = callContinuationOrder[:0]
	for _, path := range callContinuationOrder {
		if on[path] {
			order = append(order, path)
		} else {
			delete(callContinuations, path)
		}
	}
	callContinuationOrder = order

	trimCallRevisions(time.Now())
	callsMu.Unlock()
	if moved {
		// an older version's shards are never read again
		olds, _ := filepath.Glob(filepath.Join(filepath.Dir(root), "calls-v*"))
		for _, old := range olds {
			if old != root {
				_ = os.RemoveAll(old)
			}
		}
	}
	// IO never holds the metadata lock. A grace period protects live writers.
	entries, _ := os.ReadDir(root)
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		orphan := strings.HasSuffix(e.Name(), ".gob") && !shards[e.Name()]
		if strings.HasPrefix(e.Name(), "calls-") && strings.HasSuffix(e.Name(), ".tmp") {
			if st, err := e.Info(); err == nil && time.Since(st.ModTime()) > time.Hour {
				orphan = true
			}
		}
		if orphan {
			_ = os.Remove(filepath.Join(root, e.Name()))
		}
	}
}

func keepCalls(path string, st *callFile) {
	defer trimCallRevisions(time.Now())
	callCounts[path] = len(st.Calls)
	delete(callCache, path)
	order := callOrder[:0]
	for _, p := range callOrder {
		if p != path {
			order = append(order, p)
		}
	}
	callOrder = order
	if len(st.Calls) > maxKeptCalls {
		return
	}
	n := len(st.Calls)
	for _, s := range callCache {
		n += len(s.Calls)
	}
	for len(callOrder) > 0 && (n > maxKeptCalls || len(callOrder) >= maxKeptFiles) {
		p := callOrder[0]
		n -= len(callCache[p].Calls)
		delete(callCache, p)
		callOrder = callOrder[1:]
	}
	callCache[path] = st
	callOrder = append(callOrder, path)
}

// wholeCallReaders read an agent's calls from one session file at a time,
// rather than a line at a time: its calls are not lines, or its file cannot
// be read on from the middle of one. Every agent in callSources that is not
// in callFiles needs an entry here, in lineCallReaders or in
// resumeCallReaders, or its calls are never read.
var wholeCallReaders = map[string]func(file) (*callFile, bool){
	"opencode":  readOpenCodeCalls,
	"zcode":     readZCodeCalls,
	"workbuddy": readWorkBuddyCalls,
	"codebuddy": readWorkBuddyCalls, // #1779
	// #1605
	"minimax-code": readMiniMaxCalls,
	"agy":          readAgyCalls, // #2097
}

// resumeCallReaders read an agent's calls from a file only appended to that
// the line reader can't read on through, as dsh's packed file of zstd
// frames: each is handed what the last read came to and reads on from
// where it left off, or whole when it can't. It gives how many of the
// calls it hands back were in old, kept as they were (0 when it read
// whole, or what it had counted no longer stands), so only the rest are
// appended to the disk shard.
var resumeCallReaders = map[string]func(f file, old *callFile) (st *callFile, kept int, ok bool){
	"dsh": readDshCalls,
}

// lineCallReader reads an agent's calls a line at a time, as Claude Code's
// and Codex's are: a file that only grew is read on from where the last read
// left off (prepareCalls), and only the calls it adds are written to the
// disk shard. begin starts a file read from its start; line reads one whole
// line of it. Nothing a line leaves for the next may live outside st. kept,
// when a reader carries something from line to line, says whether st holds
// it: a read kept on disk before it was carried there does not, and is
// read whole again.
type lineCallReader struct {
	begin func(st *callFile, f file)
	line  func(st *callFile, b []byte)
	kept  func(st *callFile) bool
}

// lineCallReaders are the agents in callSources, beside callFiles' own,
// whose calls are each one line of a file only appended to.
var lineCallReaders = map[string]lineCallReader{
	"kimi": {kimiCallsBegin, kimiCallLine, nil},         // #1607
	"pi":   {piCallsBegin, readPiCallLine, piCallsKept}, // #1951
	"omp":  {piCallsBegin, readPiCallLine, piCallsKept},
}

// callLock is the lock a file's calls are read under, so that two readers
// never read the same file at once.
func callLock(path string) *sync.Mutex {
	h := fnv.New32a()
	_, _ = h.Write([]byte(path))
	return &callIO[h.Sum32()%uint32(len(callIO))]
}

func readCalls(f file) *callFile {
	lock := callLock(f.path)
	lock.Lock()
	defer lock.Unlock()
	callsMu.Lock()
	old, generation := callCache[f.path], callGeneration
	callsMu.Unlock()
	if old == nil {
		old = restoreCallContinuation(loadCalls(f))
	}
	if old != nil && old.Size == f.size && old.Mod == f.mod.UnixNano() {
		callsMu.Lock()
		if generation == callGeneration {
			keepCalls(f.path, old)
		}
		callsMu.Unlock()
		return old
	}
	if f.cold || wslCold(f.path) {
		// in a stopped WSL distro: opening it would start it
		if old != nil {
			return old
		}
		return &callFile{Path: f.path, Agent: f.agent}
	}
	// a file to read: few at once while only the warm-up asks (quiet.go)
	defer readSlot()()
	defer readBegin()()
	// Agents whose calls are read whole rather than line by line: a session
	// that changed is read again, and one that did not is kept. OpenCode's
	// and ZCode's are rows of a database or JSON documents; dsh's file is
	// packed in frames, so it cannot be read on from the middle of one.
	if read, ok := resumeCallReaders[f.agent]; ok {
		st, kept, readOK := read(f, old)
		if !readOK && old != nil {
			return old
		}
		if readOK {
			st.Size, st.Mod = f.size, f.mod.UnixNano()
			writeCalls(st, kept, kept > 0)
		}
		callsMu.Lock()
		if readOK && generation == callGeneration {
			keepCalls(f.path, st)
		}
		callsMu.Unlock()
		return st
	}
	if read, ok := wholeCallReaders[f.agent]; ok {
		st, readOK := read(f)
		if !readOK && old != nil {
			return old // not readable now: what was read before
		}
		if readOK {
			st.Size, st.Mod = f.size, f.mod.UnixNano()
			writeCalls(st, 0, false)
		}
		callsMu.Lock()
		if readOK && generation == callGeneration {
			keepCalls(f.path, st)
		}
		callsMu.Unlock()
		return st
	}
	if r, ok := lineCallReaders[f.agent]; ok && r.kept != nil && old != nil && !r.kept(old) {
		old = nil
	}
	st := prepareCalls(f, old)
	continued := old != nil && st.Off == old.Off && st.Off > 0
	start := 0
	if continued {
		start = len(old.Calls)
	}
	line := claudeCallLine
	if f.agent == "codex" {
		line = codexCallLine
	} else if r, ok := lineCallReaders[f.agent]; ok {
		if st.Off == 0 {
			r.begin(st, f)
		}
		line = r.line
	}
	off, err := scanAt(f.path, st.Off, func(b []byte) bool { return callHead(st, b) }, func(b []byte, start, end int64) bool {
		st.At, st.End = start, end
		line(st, b)
		return true
	})
	if err == nil {
		st.Off, st.Size, st.Mod = off, f.size, f.mod.UnixNano()
		// the bytes read, not the listed size: a listing may fold another
		// file's size into this one's (Kimi's state.json), and hashing past
		// the end would leave no hash to read on from
		st.ContentHash = prefixHash(f.path, off)
		writeCalls(st, start, continued)
	}
	// Do not turn a one-time historical scan into an indefinitely retained
	// window. Previously retained windows may survive idle time in spare slots.
	if st.Claude != nil && (old == nil || old.Claude == nil) && !workingRevision(f.mod.UnixNano(), time.Now()) {
		st = st.withoutRevisions()
	}

	st.dirty = nil
	callsMu.Lock()
	if generation == callGeneration {
		keepCalls(f.path, st)
	}
	callsMu.Unlock()
	return st
}

// callsRide reads a packed Codex rollout's calls on the scan that parses its
// summary. A packed file is decompressed whole for every read, and when the
// Codex app packs or repacks every rollout (11,591 on 10-20) the start's
// warm-up parsed each for the Sessions page and then read each again for
// Requests' calls: ~48 GB decompressed twice. Reading both on one pass
// leaves readCalls a shard on disk that is already up to date.
type callsRide struct {
	f          file
	st         *callFile
	generation uint64
	lock       *sync.Mutex
	sum, calls bool // what each reader's head said of the line being read
}

// callsAlong is a ride for f's calls when f is a packed Codex rollout whose
// calls are to be read again, and nil otherwise. It holds the file's calls
// lock until release, so a readCalls of the file meanwhile waits for what
// the ride reads rather than read it again.
func callsAlong(f file) *callsRide {
	if f.agent != "codex" || !packed(f.path) || f.cold || wslCold(f.path) {
		return nil
	}
	lock := callLock(f.path)
	lock.Lock()
	callsMu.Lock()
	old, generation := callCache[f.path], callGeneration
	callsMu.Unlock()
	if old == nil {
		old = restoreCallContinuation(loadCalls(f))
	}
	if old != nil && old.Size == f.size && old.Mod == f.mod.UnixNano() {
		lock.Unlock()
		return nil
	}
	return &callsRide{f: f, st: prepareCalls(f, old), generation: generation, lock: lock}
}

// ride is a scan's head and line for both readers: each head sees every
// line, as on a scan of its own, and each line goes to the readers whose
// head wanted it.
func (r *callsRide) ride(head func([]byte) bool, line func([]byte, int64, int64) bool) (func([]byte) bool, func([]byte, int64, int64) bool) {
	return func(b []byte) bool {
			r.sum = head == nil || head(b)
			r.calls = callHead(r.st, b)
			return r.sum || r.calls
		}, func(b []byte, start, end int64) bool {
			if r.sum {
				line(b, start, end)
			}
			if r.calls {
				r.st.At, r.st.End = start, end
				codexCallLine(r.st, b)
			}
			return true
		}
}

// release lets the file's calls lock go.
func (r *callsRide) release() { r.lock.Unlock() }

// finish keeps the calls read, as readCalls would have, when the scan
// read to its end; after a failed one readCalls reads them itself.
func (r *callsRide) finish(off int64, err error) {
	if err != nil {
		return
	}
	st, f := r.st, r.f
	st.Off, st.Size, st.Mod = off, f.size, f.mod.UnixNano()
	st.ContentHash = prefixHash(f.path, off)
	writeCalls(st, 0, false)
	st.dirty = nil
	callsMu.Lock()
	if r.generation == callGeneration {
		keepCalls(f.path, st)
	}
	callsMu.Unlock()
}

// Common strings are stored once per append frame; File is implied by State.
type callPatch struct {
	Index int
	Call  Call
	Text  [13]uint32
	Began time.Time
}
type callFrame struct {
	State   callDiskState
	Strings []string
	Patches []callPatch
}

// Disk state contains only scalar continuation metadata. Even a nil runtime
// index makes gob emit its recursive type descriptors in every append frame.
// Keep those types entirely outside the wire schema.
type callDiskState struct {
	ContentHash        string
	Size, Mod, Off     int64
	Head               string
	HeadSize           int
	Path, Agent        string
	At, End, AsstEnd   int64
	Session, Requested string
	LastUser, LastAsst time.Time
	// dsh's carry (dshCarry), as JSON: nil when there is none
	Dsh []byte
	// Pi's and omp's carry (piCarry), flat: PiOn when there is one
	PiOn, PiHeader      bool
	PiCwd               string
	PiSince             time.Time
	PiProvider, PiModel string
}

func diskCallState(s *callFile) callDiskState {
	d := callDiskState{ContentHash: s.ContentHash, Size: s.Size, Mod: s.Mod, Off: s.Off, Head: s.Head, HeadSize: s.HeadSize,
		Path: s.Path, Agent: s.Agent, At: s.At, End: s.End, AsstEnd: s.AsstEnd, Session: s.Session, Requested: s.Requested,
		LastUser: s.LastUser, LastAsst: s.LastAsst}
	if s.Dsh != nil {
		d.Dsh, _ = json.Marshal(s.Dsh)
	}
	if p := s.Pi; p != nil {
		d.PiOn, d.PiHeader, d.PiCwd, d.PiSince, d.PiProvider, d.PiModel = true, p.Header, p.Cwd, p.Since, p.Provider, p.Model
	}
	return d
}
func (s callDiskState) restore() *callFile {
	st := &callFile{ContentHash: s.ContentHash, Size: s.Size, Mod: s.Mod, Off: s.Off,
		Head: s.Head, HeadSize: s.HeadSize, Path: s.Path, Agent: s.Agent, At: s.At, End: s.End,
		AsstEnd: s.AsstEnd, Session: s.Session, Requested: s.Requested, LastUser: s.LastUser, LastAsst: s.LastAsst}
	if len(s.Dsh) > 0 {
		var c dshCarry
		if json.Unmarshal(s.Dsh, &c) == nil {
			st.Dsh = &c
		}
	}
	if s.PiOn {
		st.Pi = &piCarry{s.PiHeader, s.PiCwd, s.PiSince, s.PiProvider, s.PiModel}
	}
	return st
}

func callText(c *Call) [13]*string {
	return [13]*string{&c.Agent, &c.Session, &c.ThreadID, &c.Model, &c.Requested, &c.Effort, &c.Cwd, &c.Upstream, &c.AccountID, &c.UserID, &c.Error, &c.ErrorText, &c.File}
}

func loadCalls(f file) *callFile {
	in, err := os.Open(callCachePath(f.path))
	if err != nil {
		return nil
	}
	defer in.Close()
	var st *callFile
	var calls []Call
	msgs := map[string]int{}
	began := map[string]time.Time{}
	for {
		var header [8]byte
		_, err = io.ReadFull(in, header[:])
		if err == io.EOF {
			break
		}
		if err != nil {
			return nil
		}
		size := binary.LittleEndian.Uint32(header[:4])
		if size == 0 || size > 256<<20 {
			return nil
		}
		data := make([]byte, int(size))
		if _, err = io.ReadFull(in, data); err != nil || crc32.ChecksumIEEE(data) != binary.LittleEndian.Uint32(header[4:]) {
			return nil
		}
		var frame callFrame
		if gob.NewDecoder(bytes.NewReader(data)).Decode(&frame) != nil || frame.State.Path != f.path || frame.State.Agent != f.agent {
			return nil
		}
		if st != nil && frame.State.Off < st.Off {
			return nil
		}
		// a frame's patches are mostly new calls: room for them at once,
		// rather than append's doublings, which a warm start paid ~500 MB
		// for across every cached file
		calls = slices.Grow(calls, len(frame.Patches))
		for _, p := range frame.Patches {
			if p.Index < 0 || p.Index > len(calls) {
				return nil
			}
			for i, field := range callText(&p.Call) {
				if int(p.Text[i]) >= len(frame.Strings) {
					return nil
				}
				*field = frame.Strings[p.Text[i]]
			}
			p.Call.File = f.path
			if p.Index == len(calls) {
				calls = append(calls, p.Call)
			} else {
				calls[p.Index] = p.Call
			}
			if p.Call.Msg != "" {
				msgs[p.Call.Msg] = p.Index
				if !p.Began.IsZero() {
					began[p.Call.Msg] = p.Began
				}
			}
		}
		st = frame.State.restore()
	}
	if st == nil {
		return nil
	}
	st.Calls, st.Msgs, st.Began = calls, msgs, began
	return st
}

func writeCalls(st *callFile, start int, appendOnly bool) {
	frame := callFrame{State: diskCallState(st), Strings: []string{""}}
	dict := map[string]uint32{"": 0}
	add := func(i int) {
		p := callPatch{Index: i, Call: st.Calls[i], Began: st.Began[st.Calls[i].Msg]}
		p.Call.File = ""
		for j, field := range callText(&p.Call) {
			id, ok := dict[*field]
			if !ok {
				id = uint32(len(frame.Strings))
				dict[*field] = id
				frame.Strings = append(frame.Strings, *field)
			}
			p.Text[j] = id
			*field = ""
		}
		frame.Patches = append(frame.Patches, p)
	}
	for i := 0; i < start; i++ {
		if st.dirty[i] {
			add(i)
		}
	}
	for i := start; i < len(st.Calls); i++ {
		add(i)
	}
	var data bytes.Buffer
	if gob.NewEncoder(&data).Encode(frame) != nil {
		return
	}
	var header [8]byte
	binary.LittleEndian.PutUint32(header[:4], uint32(data.Len()))
	binary.LittleEndian.PutUint32(header[4:], crc32.ChecksumIEEE(data.Bytes()))
	dir := callCacheDir()
	if os.MkdirAll(dir, 0700) != nil {
		return
	}
	var out *os.File
	var err error
	if appendOnly {
		out, err = os.OpenFile(callCachePath(st.Path), os.O_WRONLY|os.O_APPEND, 0600)
	} else {
		out, err = os.CreateTemp(dir, "calls-*.tmp")
	}
	if err != nil {
		return
	}
	if !appendOnly {
		defer os.Remove(out.Name())
	}
	_, err = out.Write(header[:])
	if err == nil {
		_, err = out.Write(data.Bytes())
	}
	if closeErr := out.Close(); err == nil {
		err = closeErr
	}
	if err == nil && !appendOnly {
		_ = os.Rename(out.Name(), callCachePath(st.Path))
	}
}
