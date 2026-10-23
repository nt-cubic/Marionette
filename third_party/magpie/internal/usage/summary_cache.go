package usage

import (
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"slices"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

const summaryCacheSessions = 1024

type summaryEntry struct {
	version uint64
	meta    string
	// value keeps the top summaryCacheSessions sessions only, of its
	// SessionCount
	value Summary
}

// summaryKey is a period as read in a zone: each reader's zone (Zone) has
// days of its own, and readers in two zones don't put each other's out.
type summaryKey struct {
	p    Period
	zone *time.Location
}

var summaries struct {
	sync.Mutex
	entries map[summaryKey]summaryEntry
	worked  int // how many summaries were worked out, for the tests
}

func indexedSummary(p Period, now time.Time) Summary { return topSummary(p, now, -1) }

// SummarizeTop is Summarize with only the top n sessions by tokens (all of
// them for n < 0); SessionCount says how many the period has. It is what a page that
// shows a few of them or none asks for: a summary of more than
// summaryCacheSessions sessions is kept with those only, and Summarize, which
// gives every one, works it out again each time. Over a long history
// (Vantiboo1ean, ~750k requests in ~84k sessions) that was every refresh of
// the Usage page and the TUI, and the page was sent all 84k.
func SummarizeTop(p Period, n int) Summary { return topSummary(p, Clock(), n) }

// SummarizeTopIn is SummarizeTop with the period's days and the chart's
// hours read in zone (Zone gives it), the reader's: the Usage page's.
func SummarizeTopIn(p Period, n int, zone *time.Location) Summary {
	if zone == nil {
		zone = time.Local
	}
	return topSummary(p, Clock().In(zone), n)
}

func topSummary(p Period, now time.Time, n int) Summary {
	p = p.shown()
	snapshot := readLogSnapshot()
	work := func() Summary {
		summaries.Lock()
		summaries.worked++
		summaries.Unlock()
		return summarizeFrom(p, now, snapshot.first, snapshot.keyProviders, func(fn func(Record)) { snapshot.visit(p.Since(now), fn) })
	}
	if p.IsRange() || p.Last() > 0 {
		// a picked range is read afresh each time, so ranges never pile up
		// in the cache (#1492), and so is a rolling window, whose start
		// moves with the clock though the log hasn't changed (#2075)
		return topOf(work(), n)
	}
	meta := usagePriceCacheKey(now)
	summaries.Lock()
	k := summaryKey{p, now.Location()}
	old, ok := summaries.entries[k]
	summaries.Unlock()
	if ok && old.version == snapshot.version && old.meta == meta && (n >= 0 && n <= len(old.value.Sessions) || old.value.SessionCount == len(old.value.Sessions)) {
		return topOf(cloneSummary(old.value), n)
	}
	value := work()
	// the cache keeps a copy with the top sessions only; the answer has as
	// many as asked
	kept := value
	kept.Sessions = kept.Sessions[:min(len(kept.Sessions), summaryCacheSessions)]
	kept = cloneSummary(kept)
	summaries.Lock()
	if summaries.entries == nil {
		summaries.entries = map[summaryKey]summaryEntry{}
	}
	if previous := summaries.entries[k]; previous.version <= snapshot.version {
		summaries.entries[k] = summaryEntry{snapshot.version, meta, kept}
	}
	summaries.Unlock()
	return topOf(value, n)
}

// topOf is s with its top n sessions (all for n < 0).
func topOf(s Summary, n int) Summary {
	if n >= 0 && len(s.Sessions) > n {
		s.Sessions = s.Sessions[:n:n]
	}
	return s
}

// The exported result remains owned by its caller.
func cloneSummary(s Summary) Summary {
	s.Series = slices.Clone(s.Series)
	s.Agents = slices.Clone(s.Agents)
	s.Models = slices.Clone(s.Models)
	s.ProviderKeys = slices.Clone(s.ProviderKeys)
	s.Accounts = slices.Clone(s.Accounts)
	s.CallerKeys = slices.Clone(s.CallerKeys)
	s.Sessions = slices.Clone(s.Sessions)
	return s
}

// usagePriceCacheKey invalidates priced views when local prices or provider feeds change.
func usagePriceCacheKey(now time.Time) string {
	_, offset := now.Zone()
	meta := fmt.Sprintf("%s|%s|%s|%s|%s|%s|%d", statKey(settings.Path()), statKey(provider.Path()), statKey(catalog.CachePath()), statKey(catalog.LivePath("antigravity")), now.Format("2006-01-02"), now.Location(), offset)
	// Sign-ins and plugin lists can change independently of providers.json.
	// Their identity and price feeds affect both grouping and historical costs.
	h := sha256.New()
	prices, _ := json.Marshal(settings.Load().ModelPrices)
	aliases, _ := json.Marshal(provider.Renamed())
	fmt.Fprint(h, string(prices), string(aliases))
	for _, current := range provider.All() {
		fmt.Fprintf(h, "%q:%q:%v:%t:%t:%t;", current.ID, current.Where(), current.Catalogs(), current.IsPlugin(), current.Account == nil, current.Key != "")
		fmt.Fprint(h, statKey(catalog.LivePath(current.ID)))
	}
	meta += fmt.Sprintf("|%x", h.Sum(nil))
	// and the clients the user counts as agents (#1958)
	return meta + "|" + clientAgentsKey()
}
