// Package budget holds each gateway key to its own limit (#585): what a
// key has used in its window is summed from usage.jsonl, the records the
// Usage page shows, so it survives a restart; a request is let in only
// while the key has something left after what its requests in flight are
// expected to use, and those are settled with what the call recorded.
//
// The rules:
//   - A call counts in the window it started in (its record's time).
//     Windows are calendar ones in local time, or cycles of N days from
//     when the limit was set (access.Limit.Window). A reset (#1509)
//     starts the window again from then: what came before it doesn't
//     count, tokens and cost alike.
//   - Tokens are a call's uncached input, output and cache writes, and
//     its cache reads when the limit says so; cost is an estimate at the
//     Usage page's prices, and a call with no known price adds none.
//   - A request refused before any upstream (no model, a refusal like
//     this one) uses nothing; a failed call counts the tokens it recorded.
//   - A streamed reply is settled when it ends, with the usage its vendor
//     reported; one cut off before that counts what was recorded.
//   - While a request is in flight it holds a reservation: its body's
//     size in tokens (4 bytes a token) plus the key's mean output per
//     call this window. A request is let in while used + reserved is
//     under the cap, so parallel requests overshoot by about one call.
//   - A key can have several limits, each in its own window (#2227: 5
//     hours and a week, as a subscription's): a request is let in only
//     while every one has something left, and its reservation is held
//     in each window.
//   - Only requests that came with a gateway key are limited: from
//     another computer one is required; on this one a request without one
//     (or with a token that is no key) isn't limited.
package budget

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/gwbudget"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// sums is what a key's calls came to in a window.
type sums struct {
	start                                time.Time
	calls                                int
	input, output, cacheRead, cacheWrite int64
	cost                                 float64
	unpriced                             int
}

func (s *sums) add(r usage.Record, cost float64, priced bool) {
	s.calls++
	s.input += int64(r.Input)
	s.output += int64(r.Output)
	s.cacheRead += int64(r.CacheRead)
	s.cacheWrite += int64(r.CacheWrite)
	s.cost += cost
	if !priced {
		s.unpriced++
	}
}

func (s *sums) tokens(l *access.Limit) int64 {
	n := s.input + s.output + s.cacheWrite
	if l != nil && l.CacheReads {
		n += s.cacheRead
	}
	return n
}

type held struct {
	n      int
	tokens int64
	cost   float64
}

// A reservation stays in the window where the request began, even if it
// finishes after the next one has started.
type window struct {
	key   string
	start int64 // the window's start as Unix seconds
}

var state = struct {
	sync.Mutex
	sums map[string][]*sums // by usage.Path() and key, one a window its limits count in
	held map[window]*held
}{sums: map[string][]*sums{}, held: map[window]*held{}}

func slot(keyID string) string { return usage.Path() + "\x00" + keyID }

// counts reports whether r is a call a key's budget counts.
// A call an app reported itself (#1874) never counts: it didn't go through
// magpie, and may be a backfill of calls long past.
func counts(r usage.Record) bool { return !r.IsRejected() && !r.IsReported() && r.Computer == "" }

// sumsFor is keyID's sums for each window starting at starts, read from
// usage.jsonl the first time; those of windows no limit counts in now
// are let go. Called with state held.
func sumsFor(keyID string, starts []time.Time) []*sums {
	k := slot(keyID)
	kept := state.sums[k]
	out := make([]*sums, len(starts))
	var next []*sums
	for i, start := range starts {
		j := slices.IndexFunc(kept, func(s *sums) bool { return s.start.Equal(start) })
		if j < 0 {
			j = slices.IndexFunc(next, func(s *sums) bool { return s.start.Equal(start) })
			if j >= 0 {
				out[i] = next[j]
				continue
			}
			out[i] = read(keyID, start)
		} else {
			out[i] = kept[j]
		}
		next = append(next, out[i])
	}
	state.sums[k] = next
	return out
}

func read(keyID string, start time.Time) *sums {
	s := &sums{start: start}
	cost := usage.NewCoster()
	usage.Visit(start, func(r usage.Record) {
		if r.CallerKeyID == keyID && counts(r) && !r.Time.Before(start) {
			c, ok := cost(r)
			s.add(r, c, ok)
		}
	})
	return s
}

// Forget drops what is kept in memory, as a restart would.
func Forget() {
	state.Lock()
	defer state.Unlock()
	state.sums = map[string][]*sums{}
	state.held = map[window]*held{}
}

// Append writes rec to usage.jsonl and, for a key's call, counts it
// against the key's window. The two happen together so a window read from
// the file meanwhile counts the call once.
func Append(rec usage.Record) {
	if rec.CallerKeyID == "" {
		usage.Append(rec)
		return
	}
	state.Lock()
	defer state.Unlock()
	if rec.Time.IsZero() {
		rec.Time = time.Now()
	}
	usage.Append(rec)
	kept := state.sums[slot(rec.CallerKeyID)]
	if len(kept) == 0 || !counts(rec) {
		return
	}
	c, ok := usage.NewCoster()(rec)
	for _, s := range kept {
		if !rec.Time.Before(s.start) {
			s.add(rec, c, ok)
		}
	}
}

// Status is a key's limit and what it has used of it.
type Status struct {
	Period string `json:"period"`
	// Days is a "days" period's cycle, Hours an "hours" one's
	Days     int       `json:"days,omitempty"`
	Hours    int       `json:"hours,omitempty"`
	Start    time.Time `json:"start"`
	Reset    time.Time `json:"reset"`
	Calls    int       `json:"calls"`
	Tokens   int64     `json:"tokens"`
	Cost     float64   `json:"cost"`
	Unpriced int       `json:"unpriced,omitempty"`
	// InFlight requests, and the tokens and cost they are reserved at
	InFlight     int     `json:"inFlight,omitempty"`
	Reserved     int64   `json:"reserved,omitempty"`
	ReservedCost float64 `json:"reservedCost,omitempty"`
	TokenLimit   int64   `json:"tokenLimit,omitempty"`
	CostLimit    float64 `json:"costLimit,omitempty"`
	CacheReads   bool    `json:"cacheReads,omitempty"`
	// TokensLeft and CostLeft are what is left of each cap, 0 at most
	TokensLeft int64   `json:"tokensLeft"`
	CostLeft   float64 `json:"costLeft"`
	// Spent: the key is refused until Reset
	Spent bool `json:"spent"`
}

func status(keyID string, l *access.Limit, s *sums, h *held, reset time.Time) Status {
	st := Status{Period: l.Period, Days: l.Days, Hours: l.Hours, Start: s.start, Reset: reset, Calls: s.calls, Tokens: s.tokens(l), Cost: s.cost, Unpriced: s.unpriced,
		TokenLimit: l.Tokens, CostLimit: l.Cost, CacheReads: l.CacheReads}
	if h != nil {
		st.InFlight, st.Reserved, st.ReservedCost = h.n, h.tokens, h.cost
	}
	if l.Tokens > 0 {
		st.TokensLeft = max(l.Tokens-st.Tokens, 0)
		st.Spent = st.Tokens >= l.Tokens
	}
	if l.Cost > 0 {
		st.CostLeft = max(l.Cost-st.Cost, 0)
		st.Spent = st.Spent || st.Cost >= l.Cost
	}
	return st
}

// Of is key k's status now of its first limit, nil when it has no limit.
// It reads the file afresh rather than the gateway's sums, so it is right
// for a gateway served by another process too.
func Of(k access.Key, now time.Time) *Status {
	all := All(k, now)
	if len(all) == 0 {
		return nil
	}
	return &all[0]
}

// All is key k's status now of each of its limits (#2227), in the
// order k keeps them; none for a key with no limit. A window two
// limits share is read once.
func All(k access.Key, now time.Time) []Status {
	ls := k.Limits()
	var out []Status
	read1 := map[int64]*sums{}
	for _, l := range ls {
		start, reset := l.Window(now)
		s := read1[start.Unix()]
		if s == nil {
			s = read(k.ID, start)
			read1[start.Unix()] = s
		}
		state.Lock()
		h := state.held[window{slot(k.ID), start.Unix()}]
		out = append(out, status(k.ID, l, s, h, reset))
		state.Unlock()
	}
	return out
}

// Binding is the one of a key's statuses that holds it most: of those
// spent, the one that resets last, as the key is refused until then;
// else the one with the most of a cap used. Nil for none.
func Binding(all []Status) *Status {
	var b *Status
	for i := range all {
		st := &all[i]
		switch {
		case b == nil:
			b = st
		case st.Spent != b.Spent:
			if st.Spent {
				b = st
			}
		case st.Spent:
			if st.Reset.After(b.Reset) {
				b = st
			}
		case st.share() > b.share():
			b = st
		}
	}
	return b
}

// share is how much of its fuller cap st has used, 0 to 1 and over.
func (st *Status) share() float64 {
	var f float64
	if st.TokenLimit > 0 {
		f = float64(st.Tokens) / float64(st.TokenLimit)
	}
	if st.CostLimit > 0 {
		f = max(f, st.Cost/st.CostLimit)
	}
	return f
}

// Refusal is why a key's request was turned away.
type Refusal struct {
	Status
	Key string
}

// Error says it as a client is told: the key, what it used of which
// cap, and when it resets.
func (r *Refusal) Error() string {
	what := ""
	if r.TokenLimit > 0 && r.Tokens+r.Reserved >= r.TokenLimit {
		counted := "input, output and cache-write tokens"
		if r.CacheReads {
			counted = "input, output and cache tokens"
		}
		what = fmt.Sprintf("%d of its %d %s", r.Tokens, r.TokenLimit, counted)
	} else {
		what = fmt.Sprintf("$%.2f of its $%.2f estimated cost", r.Cost, r.CostLimit)
	}
	if !r.Spent {
		what += fmt.Sprintf(", with %d requests in flight holding the rest", r.InFlight)
	}
	return fmt.Sprintf("magpie gateway key %q has used %s for this %s; it resets at %s",
		r.Key, what, (&access.Limit{Period: r.Period, Days: r.Days, Hours: r.Hours}).Per(), r.Reset.Format("2006-01-02 15:04 MST"))
}

// RetryAfter is how long until the window resets, in whole seconds.
func (r *Refusal) RetryAfter(now time.Time) int {
	return max(int(r.Reset.Sub(now).Seconds()+0.999), 1)
}

// ResetAt is when the window resets.
func (r *Refusal) ResetAt() time.Time { return r.Reset }

// Limiter is Reserve as the gateway's budget point asks it
// (gwbudget.Limiter); plugins/budget serves it.
type Limiter struct{}

func (Limiter) Reserve(who access.Identity, body int64, model string, now time.Time) (func(), gwbudget.Refusal) {
	release, refused := Reserve(who, body, model, now)
	if refused != nil {
		return nil, refused
	}
	return release, nil
}

// Reserve lets a request of who's in, holding a reservation for it until
// the release it returns is called, or refuses it. body is the request's
// size in bytes (-1 unknown) and model the model it named, for a cost cap
// before the key has a price of its own this window.
func Reserve(who access.Identity, body int64, model string, now time.Time) (release func(), refused *Refusal) {
	ls := who.Limits()
	if who.KeyID == "" || len(ls) == 0 {
		return func() {}, nil
	}
	starts := make([]time.Time, len(ls))
	resets := make([]time.Time, len(ls))
	for i, l := range ls {
		starts[i], resets[i] = l.Window(now)
	}
	state.Lock()
	defer state.Unlock()
	ss := sumsFor(who.KeyID, starts)
	heldAt := func(start time.Time) *held { return state.held[window{slot(who.KeyID), start.Unix()}] }
	// every limit is asked: of those with nothing left, the one that
	// resets last is the one the client is told, as it is refused until then
	for i, l := range ls {
		s, h := ss[i], heldAt(starts[i])
		var ht int64
		var hc float64
		if h != nil {
			ht, hc = h.tokens, h.cost
		}
		if (l.Tokens > 0 && s.tokens(l)+ht >= l.Tokens) || (l.Cost > 0 && s.cost+hc >= l.Cost) {
			if refused == nil || resets[i].After(refused.Reset) {
				refused = &Refusal{Status: status(who.KeyID, l, s, h, resets[i]), Key: who.KeyName}
			}
		}
	}
	if refused != nil {
		return nil, refused
	}
	// one reservation a window, however many of the key's limits count
	// in it, at that window's pace
	type hold struct {
		k    window
		h    *held
		est  int64
		cost float64
	}
	var holds []hold
	for i, l := range ls {
		k := window{slot(who.KeyID), starts[i].Unix()}
		j := slices.IndexFunc(holds, func(x hold) bool { return x.k == k })
		if j < 0 {
			s := ss[i]
			est := max(body, 0) / 4
			if s.calls > 0 {
				est += s.output / int64(s.calls)
			}
			holds = append(holds, hold{k: k, est: est})
			j = len(holds) - 1
		}
		x := &holds[j]
		if l.Cost > 0 && x.cost == 0 {
			s, used := ss[i], ss[i].tokens(l)
			if used > 0 && s.cost > 0 {
				x.cost = float64(x.est) * s.cost / float64(used)
			} else if p, ok := priceOf(model); ok {
				x.cost = float64(x.est) * p
			}
		}
	}
	for i := range holds {
		x := &holds[i]
		x.h = state.held[x.k]
		if x.h == nil {
			x.h = &held{}
			state.held[x.k] = x.h
		}
		x.h.n++
		x.h.tokens += x.est
		x.h.cost += x.cost
	}
	once := sync.Once{}
	return func() {
		once.Do(func() {
			state.Lock()
			defer state.Unlock()
			for _, x := range holds {
				x.h.n--
				x.h.tokens -= x.est
				x.h.cost -= x.cost
				if x.h.n == 0 && state.held[x.k] == x.h {
					delete(state.held, x.k)
				}
			}
		})
	}, nil
}

// priceOf is the input price per token of model when it names its
// provider ("provider/model").
func priceOf(model string) (float64, bool) {
	p, m, ok := strings.Cut(model, "/")
	if !ok || p == "" || m == "" {
		return 0, false
	}
	pr, ok := provider.EffectivePriceIn(settings.Load(), p, m)
	if !ok {
		return 0, false
	}
	return pr.Input / 1e6, true
}

// ModelOf is the model a JSON request body names, "" for none.
func ModelOf(body []byte) string {
	var v struct {
		Model string `json:"model"`
	}
	if json.Unmarshal(body, &v) != nil {
		return ""
	}
	return v.Model
}
