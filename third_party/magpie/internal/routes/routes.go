// Package routes is the gateway's router strategy point (P3 of
// docs/subsystems/plugin-runtime.md): how one pool of routes — a
// provider's accounts or keys, or the heads of a group's members — is
// ordered for a request. The gateway lists the pool, rests, affinity and
// failover itself; a Strategy only says the order.
//
// Smart, the default, is the core's. The others a provider or group can be
// routed by (order, rotate, usage, pace, weight) are here for a plugin to
// register: plugins/rotation does, and off, a pool routed by one of them is
// tried in its listed order, as an unknown routing always was.
package routes

import (
	"log"
	"slices"
	"sync"
	"time"
)

// Seat is one route of a pool, as a strategy weighs it.
type Seat struct {
	// Key is who it is: the account or key's rest key, the same for the
	// same route from request to request
	Key string
	// Account is a subscription's account, not an API key
	Account bool
	// Known: its allowance is known. Used, Soon and Pace are read from it
	Known bool
	// Learns: an account not known yet, of an agent that tells what it has
	// left as it answers (Claude Code's rate_limit_event)
	Learns bool
	// Used is the share of its allowance used that counts the model, 0–100
	Used float64
	// Soon is when its windows renew, the biggest first (Allowance.Renewal)
	Soon []time.Time
	// Pace is its week left per hour until that renews; an account not
	// known has a whole week (provider.FreshPace), a key none
	Pace float64
	// Weight is its key's weight, at least 1
	Weight int
	// Resting: it rests after a failure now
	Resting bool
	// Tokens is what magpie sent it lately, decayed by an hour's half-life
	Tokens float64
}

// Strategy orders a pool.
type Strategy struct {
	// Order is the order to try seats in: a permutation of their indexes.
	// pool names the pool (a provider's id, or "group/<id>"), for a
	// strategy that keeps a round per pool. It is asked with two seats or
	// more.
	Order func(pool string, seats []Seat) []int
	// Exact: its order stands as it says. Otherwise, of a provider's API
	// keys, those that fit the request better go first, each kept in its
	// order among those alike.
	Exact bool
	// ByUse: it weighs by Tokens, which the routing trace then shows
	ByUse bool
}

var reg = struct {
	sync.RWMutex
	m map[string][]*Strategy // routing → registered, the last one serving
}{m: map[string][]*Strategy{}}

// Register serves routing by s until remove is called. Registering one
// already served stacks: the last serves until it is removed.
func Register(routing string, s Strategy) (remove func()) {
	e := &s
	reg.Lock()
	reg.m[routing] = append(reg.m[routing], e)
	reg.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			reg.Lock()
			defer reg.Unlock()
			reg.m[routing] = slices.DeleteFunc(slices.Clone(reg.m[routing]), func(x *Strategy) bool { return x == e })
		})
	}
}

// Find is the strategy serving routing: Smart for "", else the last one
// registered; ok is false when none is.
func Find(routing string) (Strategy, bool) {
	if routing == "" {
		return Smart, true
	}
	reg.RLock()
	defer reg.RUnlock()
	l := reg.m[routing]
	if len(l) == 0 {
		return Strategy{}, false
	}
	return *l[len(l)-1], true
}

// Served says whether routing is served now.
func Served(routing string) bool {
	_, ok := Find(routing)
	return ok
}

// Apply is the order s gives seats, checked: an order that isn't a
// permutation of them, or a strategy that panics, leaves them in their
// listed order, logged. A strategy only orders, so failing it fails open:
// every route is still tried.
func Apply(s Strategy, pool string, seats []Seat) (order []int) {
	defer func() {
		if r := recover(); r != nil {
			log.Printf("routes: %s's strategy panicked: %v", pool, r)
			order = identity(len(seats))
		}
	}()
	order = s.Order(pool, seats)
	seen := make([]bool, len(seats))
	ok := len(order) == len(seats)
	for _, i := range order {
		if !ok || i < 0 || i >= len(seats) || seen[i] {
			ok = false
			break
		}
		seen[i] = true
	}
	if !ok {
		log.Printf("routes: %s's strategy gave %v for %d routes; trying them in order", pool, order, len(seats))
		return identity(len(seats))
	}
	return order
}
