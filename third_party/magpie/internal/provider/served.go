package provider

// When each subscription account, plan and key last answered a request
// through the gateway (#570): magpie quota --json and the gateway's GET
// /v1/magpie/quotas tell it as each Quota's lastServedAt, and mark the
// latest last, so a menu-bar widget can show the one in use — under
// routing Smart nobody is first for good. It is kept on disk (served.json,
// next to the providers), so the CLI, a process of its own, reads what the
// gateway noted, and a restart doesn't forget it.

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// servedKeep is how long an answer is remembered: a key or account gone
// since is dropped from the file after it.
const servedKeep = 30 * 24 * time.Hour

var servedAt = struct {
	sync.Mutex
	m    map[string]time.Time // ServedID → when it last answered
	path string               // the file m goes with: another (a test's HOME) starts afresh
	// by: the ServedID that answered last for each provider (servedOf),
	// to tell OnServedMoved when that is another one
	by map[string]string
}{m: map[string]time.Time{}, by: map[string]string{}}

// OnServedMoved sets what is told when a provider's requests are answered
// by another of its accounts or keys than the one that answered before, or
// when the user signs an agent in to another account (SwitchLogin): the
// menu bar following the account in use reads it again then, rather than
// minutes later (#1516). It is called after NoteServed lets go of its lock
// and must not block; nil tells nothing.
func OnServedMoved(f func()) {
	if f == nil {
		onServedMoved.Store(nil)
		return
	}
	onServedMoved.Store(&f)
}

var onServedMoved atomic.Pointer[func()]

func servedMoved() {
	if f := onServedMoved.Load(); f != nil {
		(*f)()
	}
}

// servedOf is the provider a ServedID is of: "codex" for "codex@a@b.c",
// "deepseek" for "deepseek#k1a2b3c".
func servedOf(id string) string {
	if i := strings.IndexAny(id, "@#"); i >= 0 {
		return id[:i]
	}
	return id
}

// ours is servedAt.m for the file at path. Called with servedAt held.
func ours(path string) map[string]time.Time {
	if servedAt.path != path {
		servedAt.path, servedAt.m, servedAt.by = path, map[string]time.Time{}, map[string]string{}
	}
	return servedAt.m
}

func servedPath() string { return filepath.Join(filepath.Dir(Path()), "served.json") }

// ServedID names who answers for p, as its Quota is found by: an account
// by its agent and user ("codex@a@b.c"), a key by its provider and the
// key's id ("deepseek#k1a2b3c"), else the provider.
func ServedID(p Provider) string {
	if a := p.Account; a != nil {
		return loginProvider(Login{Agent: a.Agent}) + "@" + strings.ToLower(a.User)
	}
	if p.Key != "" {
		return p.ID + "#" + KeyID(p.Key)
	}
	return p.ID
}

// NoteServed records that p just answered a request, at at.
func NoteServed(p Provider, at time.Time) {
	id := ServedID(p)
	path := servedPath()
	servedAt.Lock()
	moved := false
	defer func() {
		servedAt.Unlock()
		if moved {
			servedMoved()
		}
	}()
	mine := ours(path)
	if at.Before(mine[id]) {
		return
	}
	mine[id] = at
	if of := servedOf(id); servedAt.by[of] != id {
		// the first this process sees too: the menu bar may have taken
		// another for the one in use until then
		moved = true
		servedAt.by[of] = id
	}
	m := readServed(path)
	for k, t := range mine {
		if t.After(m[k]) {
			m[k] = t
		}
	}
	for k, t := range m {
		if at.Sub(t) > servedKeep {
			delete(m, k)
		}
	}
	if b, err := json.Marshal(m); err == nil && os.MkdirAll(filepath.Dir(path), 0o700) == nil {
		writeFileAtomic(path, b)
	}
}

// LastServed is when each ServedID last answered: what this process noted
// and what the file holds, whichever gateway wrote it.
func LastServed() map[string]time.Time {
	path := servedPath()
	m := readServed(path)
	servedAt.Lock()
	for k, t := range ours(path) {
		if t.After(m[k]) {
			m[k] = t
		}
	}
	servedAt.Unlock()
	for k, t := range m {
		if time.Since(t) > servedKeep {
			delete(m, k)
		}
	}
	return m
}

func readServed(path string) map[string]time.Time {
	m := map[string]time.Time{}
	if b, err := os.ReadFile(path); err == nil {
		json.Unmarshal(b, &m)
	}
	if m == nil {
		m = map[string]time.Time{}
	}
	return m
}

// withServed sets each quota's LastServedAt from served, and Last on the
// latest (on each of them, when a key's plan and balance tie).
func withServed(qs []Quota, served map[string]time.Time) []Quota {
	if len(served) == 0 {
		return qs
	}
	by := servedBy{served: served}
	var latest time.Time
	for i := range qs {
		q := &qs[i]
		if at := by.at(q.Provider, q.Kind, q.User); !at.IsZero() {
			t := at.UTC()
			q.LastServedAt = &t
			if at.After(latest) {
				latest = at
			}
		}
	}
	for i := range qs {
		if qs[i].LastServedAt != nil && qs[i].LastServedAt.Equal(latest) {
			qs[i].Last = true
		}
	}
	return qs
}

// servedBy finds when a card's account, plan or key last answered, in
// served (LastServed's map).
type servedBy struct {
	served map[string]time.Time
	keys   map[string][]KeyInfo // provider → its keys, read once, when a plan or balance asks
}

// at is when the card of provider's kind ("subscription", "plan" or
// "balance") told by user last answered; zero when it hasn't.
func (s *servedBy) at(provider, kind, user string) time.Time {
	var at time.Time
	latestOf := func(id string) {
		if t, ok := s.served[id]; ok && t.After(at) {
			at = t
		}
	}
	switch {
	case kind == "subscription" && user != "":
		latestOf(provider + "@" + strings.ToLower(user))
	case user == "":
		// the one account or key it has, or any of them: a balance
		// one account has whichever key asks
		for id, t := range s.served {
			if (id == provider || strings.HasPrefix(id, provider+"@") || strings.HasPrefix(id, provider+"#")) && t.After(at) {
				at = t
			}
		}
	default:
		// a plan's or balance's key, by the name or mask it is told by
		if s.keys == nil {
			s.keys = map[string][]KeyInfo{}
			for _, p := range All() {
				s.keys[p.ID] = p.KeyList()
			}
		}
		for _, k := range s.keys[provider] {
			if k.Name == user || k.Name == "" && k.Masked == user {
				latestOf(provider + "#" + k.ID)
			}
		}
	}
	return at
}

// cardsServed sets LastServedAt on the Usage page's cards, this
// computer's own (a remote magpie's answer there, not here), as
// withServed does on the gateway's: what the Usage page and the tray
// panel show in full, and what the menu bar's "account in use" follows,
// is the account that answered last. Each is a copy; the caches the
// cards come from are left as they were.
func cardsServed(subs, plans, balances []SubscriptionQuota, served map[string]time.Time) []SubscriptionQuota {
	out := make([]SubscriptionQuota, 0, len(subs)+len(plans)+len(balances))
	by := servedBy{served: served}
	for _, g := range []struct {
		kind string
		qs   []SubscriptionQuota
	}{{"subscription", subs}, {"plan", plans}, {"balance", balances}} {
		for _, q := range g.qs {
			if q.From == "" && len(served) > 0 {
				if at := by.at(q.Provider, g.kind, q.User); !at.IsZero() {
					t := at.UTC()
					q.LastServedAt = &t
				}
			}
			out = append(out, q)
		}
	}
	return out
}
