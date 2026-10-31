package routes

import (
	"cmp"
	"sort"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// Shares of an allowance past which an account is kept for when the others
// can't take a request: low, and all but used up — where the agent signed
// in to it is signed in to another, too (provider.SpentShare).
const (
	LowShare  = 90
	UsedShare = provider.SpentShare
)

// tier is 0 for a share with room, 1 for low, 2 for all but used up.
func tier(share float64) int {
	switch {
	case share >= UsedShare:
		return 2
	case share >= LowShare:
		return 1
	}
	return 0
}

// byLeft is how a pool's wallets go by what they have left (Seat.Left),
// as a comparison of two seats by their index: of seats whose balance is
// in one provider's one unit (Seat.LeftIn), the one with more left first;
// a seat that told none after those that did. Points and another
// vendor's points don't compare, so each kind is ordered only within the
// places its own seats take: those places are the told seats' order by
// before (nil: the pool's own order), each kind's seats are put in its
// places by what they have left, and seats of two kinds keep before's
// order between them. Before, a pool with two kinds was weighed by
// nothing (nil), and a group of several providers' accounts spent the
// small balances first (#4095, mycf_711). Nil when no seat told one.
func byLeft(seats []Seat, before func(a, b Seat) bool) func(i, j int) int {
	var told []int
	for i, s := range seats {
		if s.LeftIn != "" {
			told = append(told, i)
		}
	}
	if len(told) == 0 {
		return nil
	}
	if before != nil {
		sort.SliceStable(told, func(a, b int) bool { return before(seats[told[a]], seats[told[b]]) })
	}
	place := make([]int, len(seats))
	for i := range place {
		place[i] = len(seats) // told none: after every one that did
	}
	var kinds []string
	places := map[string][]int{}
	for p, i := range told {
		in := seats[i].LeftIn
		if _, ok := places[in]; !ok {
			kinds = append(kinds, in)
		}
		places[in] = append(places[in], p)
	}
	for _, in := range kinds {
		ps := places[in]
		ids := make([]int, len(ps))
		for k, p := range ps {
			ids[k] = told[p]
		}
		sort.SliceStable(ids, func(a, b int) bool { return seats[ids[a]].Left > seats[ids[b]].Left })
		for k, p := range ps {
			place[ids[k]] = p
		}
	}
	return func(i, j int) int { return cmp.Compare(place[i], place[j]) }
}

// fewerTokens is what Pace and LeastUsed go by after a wallet's balance:
// what magpie sent a seat lately, the least first. It sets the places
// wallets of two kinds keep between them (byLeft).
func fewerTokens(a, b Seat) bool { return a.Tokens < b.Tokens }

func identity(n int) []int {
	idx := make([]int, n)
	for i := range idx {
		idx[i] = i
	}
	return idx
}

// Smart is the default: of those with quota to spare, the one whose
// allowance renews soonest, since what it has left is lost then, while one
// renewing later keeps: the biggest window decides — the week, not the five
// hours in it — and the next one only when that renews in the same hour. An
// account with no week (Claude Enterprise's five hours alone) goes by its
// five hours, so ahead of every week but one renewing sooner; a window not
// started renews its whole span from now (#576). A Codex account that
// spends a reset about to run out by itself (auto-use on, its windows used)
// renews when that is spent, if sooner: what it has left is lost there just
// the same (#718). Only holding one, or one that runs out after the week
// renews, changes nothing. Renewing in the same hours, the one whose
// credits left lapse first (Seat.Lapse) goes first: WorkBuddy's free
// accounts all renew at the month's end, but the credits each was given
// end on their own days (zhaocai_83020 on Discord). Then, of prepaid
// wallets (Seat.Left), the one with more left (#3950). Those alike stay in their order, keeping the
// vendor's prompt cache warm. Past that, whichever has the most left, and
// one all but used up only when nothing else can take it, one that goes
// on on its credits first of those (spentBefore). Only the windows
// that count the model do: Opus's own weekly allowance being used up leaves
// Sonnet alone.
//
// One not known that tells what it has left as it answers goes first,
// once: else, kept after those known, it would never answer and never be
// known (Anthropic's usage endpoint can turn magpie away for hours).
var Smart = Strategy{Order: func(_ string, seats []Seat) []int {
	left := byLeft(seats, nil)
	var fine, low, spent []int
	for i, s := range seats {
		switch tier(s.Used) {
		case 2:
			spent = append(spent, i)
		case 1:
			low = append(low, i)
		default:
			fine = append(fine, i)
		}
	}
	sort.SliceStable(fine, func(i, j int) bool {
		a, b := seats[fine[i]], seats[fine[j]]
		if a.Learns != b.Learns {
			return a.Learns
		}
		ri, rj := a.Soon, b.Soon
		for k := 0; k < len(ri) || k < len(rj); k++ {
			var x, y time.Time // to the hour, so a few minutes don't reorder
			if k < len(ri) {
				x = ri[k].Truncate(time.Hour)
			}
			if k < len(rj) {
				y = rj[k].Truncate(time.Hour)
			}
			switch {
			case x.Equal(y):
				continue
			case x.IsZero() || y.IsZero(): // not known: after those known
				return y.IsZero()
			}
			return x.Before(y)
		}
		// renewing in the same hours: the one whose credits left lapse
		// first, to the hour; one that tells none after those that do
		x, y := a.Lapse.Truncate(time.Hour), b.Lapse.Truncate(time.Hour)
		if x.Equal(y) {
			// then a wallet with more left (#3950)
			return left != nil && left(fine[i], fine[j]) < 0
		}
		if x.IsZero() || y.IsZero() {
			return y.IsZero()
		}
		return x.Before(y)
	})
	sort.SliceStable(low, func(i, j int) bool { return seats[low[i]].Used < seats[low[j]].Used })
	sort.SliceStable(spent, func(i, j int) bool { return spentBefore(seats[spent[i]], seats[spent[j]]) })
	return append(append(fine, low...), spent...)
}}

// spentBefore orders two seats all but used up: one that goes on on its
// credits first (a Codex account holding them answers past its allowance;
// one with nothing left is refused), then the one with the most left.
func spentBefore(a, b Seat) bool {
	if a.Credits != b.Credits {
		return a.Credits
	}
	return a.Used < b.Used
}

// Ordered tries a pool in its order: the first until it can't answer.
var Ordered = Strategy{Exact: true, Order: func(_ string, seats []Seat) []int { return identity(len(seats)) }}

var turns = struct {
	sync.Mutex
	m map[string]int // pool → requests routed in turn
}{m: map[string]int{}}

// Rotate starts each request one further along the pool.
var Rotate = Strategy{Order: func(pool string, seats []Seat) []int {
	turns.Lock()
	n := turns.m[pool] % len(seats)
	turns.m[pool]++
	turns.Unlock()
	return append(identity(len(seats))[n:], identity(n)...)
}}

// LeastUsed puts a subscription, or a key that reads its own windows, by the
// share of its allowance used, as the vendor says; then, and for other
// keys, by what magpie sent it lately; prepaid wallets, which have no
// share, by what they have left first (Seat.Left). Of those all but used up, one that
// goes on on its credits goes first (spentBefore).
var LeastUsed = Strategy{ByUse: true, Order: func(_ string, seats []Seat) []int {
	left := byLeft(seats, fewerTokens)
	idx := identity(len(seats))
	sort.SliceStable(idx, func(a, b int) bool {
		sa, sb := seats[idx[a]], seats[idx[b]]
		if ta, tb := tier(sa.Used), tier(sb.Used); ta == 2 && tb == 2 && sa.Credits != sb.Credits {
			return sa.Credits
		}
		if sa.Used != sb.Used {
			return sa.Used < sb.Used
		}
		if left != nil {
			if c := left(idx[a], idx[b]); c != 0 {
				return c < 0
			}
		}
		return sa.Tokens < sb.Tokens
	})
	return idx
}}

// Pace puts a subscription by what it has left of its week per hour until
// that renews, the most first: it has the most to lose at the reset, where
// the most left alone (least used) sends a fresh account ahead of one with
// 80% left and an hour to go, and the soonest reset alone (smart) sends one
// with 3% left and an hour to go ahead of one with 90% and two. The tiers
// are Smart's: one at 90% or more of any window that counts the model — the
// five hours too, a rate cap it would be sent into before its allowance was
// read again — waits until the others can't answer, one all but used up
// until nothing else can; each tier by the share used, one all but used up
// that goes on on its credits first (spentBefore). Among the rest, one
// not known that tells what it has left as it answers goes first while it
// isn't, as in Smart, else it would never be known; then the pace, those
// alike within a tenth by what a wallet has left (Seat.Left), then by what
// magpie sent them lately, then in their
// order, keeping the vendor's prompt cache warm. An account not known
// counts as a whole week ahead of it; a key has no week, unless it reads
// its own windows (a sub2api key's limits). A Codex account that spends a
// reset about to run out by itself goes by the hours until then when that
// is sooner (Allowance.Pace, #717).
var Pace = Strategy{ByUse: true, Order: func(_ string, seats []Seat) []int {
	// the bands of pace alike within a tenth, the highest first: drawn
	// before sorting, from each band's highest down, so that sorting by
	// them is consistent — "within a tenth of each other" alone is not,
	// three paces a twelfth apart each going round in a circle. Drawn among
	// those the pace orders alone — one running low would otherwise set a
	// band's top and part two alike behind it
	var byPace []int
	for i, s := range seats {
		if tier(s.Used) == 0 && !s.Learns {
			byPace = append(byPace, i)
		}
	}
	sort.SliceStable(byPace, func(a, b int) bool { return seats[byPace[a]].Pace > seats[byPace[b]].Pace })
	band, top := make([]int, len(seats)), 0.0
	for k, i := range byPace {
		if p := seats[i].Pace; k == 0 || p < 0.9*top {
			top = p
			if k > 0 {
				band[i] = band[byPace[k-1]] + 1
				continue
			}
		} else {
			band[i] = band[byPace[k-1]]
		}
	}
	left := byLeft(seats, fewerTokens)
	idx := identity(len(seats))
	sort.SliceStable(idx, func(a, b int) bool {
		sa, sb := seats[idx[a]], seats[idx[b]]
		if ta, tb := tier(sa.Used), tier(sb.Used); ta != tb {
			return ta < tb
		} else if ta == 2 && sa.Credits != sb.Credits {
			return sa.Credits
		} else if ta > 0 {
			if sa.Used != sb.Used {
				return sa.Used < sb.Used
			}
		} else if sa.Learns != sb.Learns {
			return sa.Learns
		} else if band[idx[a]] != band[idx[b]] {
			return band[idx[a]] < band[idx[b]]
		} else if left != nil {
			// a wallet's pace is a whole week's; the one with more left
			// first (#3950)
			if c := left(idx[a], idx[b]); c != 0 {
				return c < 0
			}
		}
		return sa.Tokens < sb.Tokens
	})
	return idx
}}

// Weighted spreads a pool's requests over its seats by each one's weight,
// smoothly (Round): the one the pool's round picks now first, the others
// after it in their order, for failover.
var Weighted = Strategy{Order: func(pool string, seats []Seat) []int {
	keys, weights, rests := make([]string, len(seats)), make([]int, len(seats)), make([]bool, len(seats))
	for i, s := range seats {
		keys[i], weights[i], rests[i] = s.Key, max(s.Weight, 1), s.Resting
	}
	return First(Round(pool, keys, weights, rests), len(seats))
}}

// First is the order of n with best first and the others after it in their
// order; best below 1 leaves it as it is.
func First(best, n int) []int {
	idx := identity(n)
	if best <= 0 {
		return idx
	}
	return append(append([]int{best}, idx[:best]...), idx[best+1:]...)
}

// FreeFirst spends what costs nothing before what is paid for: the seats
// whose model its vendor serves at no cost to the subscription's allowance
// or credits go first, in their listed order, and the rest after them, also
// in their listed order. So a request whose model is free somewhere on the
// pool uses that first, and only when those can't answer does it go to one
// that spends credits or quota — nothing is left unused that cost nothing.
//
// It orders one tier only, unlike Smart: a free seat is not pushed behind a
// paid one for running low, since the free one spends nothing by running
// out of. Among the paid, the listed order stands rather than Smart's
// renewal-weighted one, because what this routing is for is the free tier;
// set a provider's own routing (Smart, in order, weekly pace) if the paid
// seats should be spent smartly among themselves — a provider or group in
// this group keeps its own routing either way.
//
// A seat whose model costs nothing nowhere is not free: it goes with the
// paid ones, so a routing set on a pool without free models behaves as In
// order.
var FreeFirst = Strategy{ByFree: true, Order: func(_ string, seats []Seat) []int {
	free, paid := make([]int, 0, len(seats)), make([]int, 0, len(seats))
	for i, s := range seats {
		if s.Free {
			free = append(free, i)
		} else {
			paid = append(paid, i)
		}
	}
	return append(free, paid...)
}}

// Builtin are the routings besides Smart and their strategies, as
// plugins/rotation registers them.
var Builtin = map[string]Strategy{
	provider.Ordered:   Ordered,
	provider.Rotate:    Rotate,
	provider.LeastUsed: LeastUsed,
	provider.Pace:      Pace,
	provider.Weighted:  Weighted,
	provider.FreeFirst: FreeFirst,
	provider.Fastest:   Fastest,
}
