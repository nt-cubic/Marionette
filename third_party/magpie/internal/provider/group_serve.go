package provider

import (
	"slices"
	"sync"
	"sync/atomic"
	"time"
)

// The user's own routing groups — those not Auto, made on the Routing page
// or with `magpie group` — are plugins/groups' to serve (P3 of
// docs/subsystems/plugin-runtime.md). In the process serving the gateway
// (GateGroups), while that plugin is off (no ServeGroups held), they are
// neither listed nor routed: Groups, FindGroup, GroupFor and the catalog
// leave them out. The groups magpie finds (Auto) are the core's either
// way, as Smart is: they are how a model several providers serve is one
// model with failover, which a new install, the groups plugin off, has.
//
// The plugin stays on while the user has a group of their own
// (NamedGroups), so off, there is none to leave out but one added since
// the plugin last looked: a lookup that finds one asks the gate's refresh
// (the gateway's plugin runtime) first, which turns the plugin on, so a
// group made a moment ago with `magpie group add` is served at once. A
// process not serving the gateway (the CLI, the TUI) lists them all.
var groupGate struct {
	sync.Mutex
	gated   int    // GateGroups calls not yet stopped
	served  int    // ServeGroups calls not yet removed
	refresh func() // the last GateGroups' refresh
}

// groupRefreshing: a refresh is under way, so a lookup it makes (a plugin's
// InUse) doesn't start another; refreshed: when the last one began, so a
// plugins.yml that can't be read, which leaves the plugin off, is asked
// again at most once a second, not on every lookup
var (
	groupRefreshing atomic.Bool
	groupRefreshed  atomic.Int64
)

// GateGroups makes the user's own groups wait on ServeGroups until stop:
// the gateway calls it as it starts serving. refresh, when not nil, is
// asked to bring the plugins up to date when a lookup gated finds a group
// of the user's own (served).
func GateGroups(refresh func()) (stop func()) {
	groupGate.Lock()
	groupGate.refresh = refresh
	groupGate.Unlock()
	return holdGate(&groupGate.gated)
}

// ServeGroups serves the user's own groups until remove: plugins/groups
// calls it while it is on.
func ServeGroups() (remove func()) {
	return holdGate(&groupGate.served)
}

// holdGate holds n until the func it gives is called. Nothing built from
// the groups is kept past one call (only the file is, while a request
// holds it), so the next lookup sees the change.
func holdGate(n *int) func() {
	groupGate.Lock()
	*n++
	groupGate.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			groupGate.Lock()
			*n--
			groupGate.Unlock()
		})
	}
}

// GroupsServed says whether the user's own groups are served here now.
func GroupsServed() bool {
	groupGate.Lock()
	defer groupGate.Unlock()
	return groupsServedLocked()
}

// served says whether the user's own groups among gs are served: gated and
// not, gs holding one asks the refresh once and looks again.
func served(gs []Group) bool {
	groupGate.Lock()
	ok, refresh := groupsServedLocked(), groupGate.refresh
	groupGate.Unlock()
	if ok || refresh == nil || !slices.ContainsFunc(gs, func(g Group) bool { return !g.Auto && !g.Hidden }) {
		return ok
	}
	now := time.Now().UnixNano()
	if now-groupRefreshed.Load() < int64(time.Second) || !groupRefreshing.CompareAndSwap(false, true) {
		return false
	}
	groupRefreshed.Store(now)
	refresh()
	groupRefreshing.Store(false)
	return GroupsServed()
}

func groupsServedLocked() bool { return groupGate.gated == 0 || groupGate.served > 0 }

// NamedGroups counts the user's own groups in providers.json, served or
// not; a file that can't be read is an error, not none.
func NamedGroups() (int, error) {
	f, err := read()
	if err != nil {
		return 0, err
	}
	n := 0
	for _, g := range f.Groups {
		if !g.Auto && !g.Hidden {
			n++
		}
	}
	return n, nil
}
