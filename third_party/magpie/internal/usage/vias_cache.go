package usage

import (
	"slices"
	"sync"
	"time"
)

// viaLists is what the gateway sent each session's calls to, and when each
// session's first call was: a list counted from any time at or before that
// first call is the same list.
type viaLists struct {
	m     map[string][]Via
	first map[string]time.Time
}

// viasCache keeps one index of the whole log, which answers every since a
// session's calls all came after, and one window for a session that had
// calls before its since. The Sessions list, each agent's manage page and
// a session's detail each ask from a since of their own; one kept since
// was built again from the whole log at almost every ask, ~280 MB a time
// over ~720k calls, on every poll of an open page (#2776).
var viasCache struct {
	sync.Mutex
	version uint64
	clients string // the clients counted as agents (#1958)
	all     *viaLists
	window  struct {
		version uint64
		clients string
		since   time.Time
		m       map[string][]Via
	}
}

// ViaIndex is Vias' answer: the one copy every caller shares, handed out
// whole and read only through Of, which gives each session's list as a
// copy of its own. Each ask cloned every session's list, ~66 MB a time
// over ~720k calls, where a page reads the few sessions it lists (#2776).
type ViaIndex struct {
	all      *viaLists
	since    time.Time
	snapshot *logSnapshot
	clients  string
}

// Of is what the gateway sent one session's calls to ("agent|id") since
// the index's time, the most calls first; nil for a session it sent none
// of.
func (x ViaIndex) Of(key string) []Via {
	vs, ok := x.all.m[key]
	if !ok {
		return nil
	}
	if x.since.IsZero() || !x.all.first[key].Before(x.since) {
		return slices.Clone(vs)
	}
	return slices.Clone(x.window()[key])
}

// Len is how many sessions have a list.
func (x ViaIndex) Len() int {
	if x.since.IsZero() {
		return len(x.all.m)
	}
	n := 0
	for _, vs := range x.all.m {
		if slices.ContainsFunc(vs, func(v Via) bool { return !v.Last.Before(x.since) }) {
			n++
		}
	}
	return n
}

// window is the lists counted from the index's since, for a session that
// had calls before it.
func (x ViaIndex) window() map[string][]Via {
	viasCache.Lock()
	defer viasCache.Unlock()
	w := &viasCache.window
	if w.m != nil && w.version == x.snapshot.version && w.clients == x.clients && w.since.Equal(x.since) {
		return w.m
	}
	m := buildVias(x.snapshot, x.since).m
	if x.snapshot.complete() {
		w.version, w.clients, w.since, w.m = x.snapshot.version, x.clients, x.since, m
	}
	return m
}

func cachedVias(since time.Time) ViaIndex {
	snapshot := readLogSnapshot()
	clients := clientAgentsKey()
	viasCache.Lock()
	defer viasCache.Unlock()
	all := viasCache.all
	if all == nil || viasCache.version != snapshot.version || viasCache.clients != clients {
		all = buildVias(snapshot, time.Time{})
		if snapshot.complete() {
			viasCache.version, viasCache.clients, viasCache.all = snapshot.version, clients, all
		}
	}
	return ViaIndex{all: all, since: since, snapshot: snapshot, clients: clients}
}

// complete is whether the snapshot reached the end of the log, so what is
// built from it can be kept for its version.
func (s *logSnapshot) complete() bool {
	return s.info != nil && s.off == s.info.Size()
}
