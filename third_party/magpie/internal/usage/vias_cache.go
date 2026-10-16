package usage

import (
	"slices"
	"sync"
	"time"
)

var viasCache struct {
	sync.Mutex
	version uint64
	since   time.Time
	clients string // the clients counted as agents (#1958)
	value   map[string][]Via
}

func cachedVias(since time.Time) map[string][]Via {
	snapshot := readLogSnapshot()
	clients := clientAgentsKey()
	viasCache.Lock()
	defer viasCache.Unlock()
	value := viasCache.value
	if value == nil || viasCache.version != snapshot.version || !viasCache.since.Equal(since) || viasCache.clients != clients {
		value = buildVias(snapshot, since)
		if snapshot.info != nil && snapshot.off == snapshot.info.Size() {
			viasCache.version, viasCache.since, viasCache.value, viasCache.clients = snapshot.version, since, value, clients
		}
	}
	out := make(map[string][]Via, len(value))
	for k, vs := range value {
		out[k] = slices.Clone(vs)
	}
	return out
}
