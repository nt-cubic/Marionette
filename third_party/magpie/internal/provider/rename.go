package provider

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"sync/atomic"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// Rename gives a provider another id, the one its models are picked by
// (<id>/<model>). What magpie keeps that names it by id follows: the
// routing groups it is in and their rules and classifiers, the other
// providers' fallbacks, which agents are shown it, and its StepFun sign-in. The old id stays
// with it (Was), so an agent still running on a config that names it the
// old way reaches it, and its usage so far is counted with it; the agents'
// own files are the agent package's to rewrite (agent.RenameProvider).
// A signed-in account keeps the id of its agent.
func Rename(from, to string) error {
	from = strings.ToLower(strings.TrimSpace(from))
	to = strings.ToLower(strings.TrimSpace(to))
	if to == "" || to != Slug(to) {
		return fmt.Errorf("a provider's id must be lowercase letters, digits and dashes, not %q", to)
	}
	if to == from {
		return nil
	}
	f, err := read()
	if err != nil {
		return err
	}
	i := slices.IndexFunc(f.Providers, func(p Provider) bool { return p.ID == from })
	// one of the user's own saved on a subscription's id before that was one
	// (a "WorkBuddy" key before v0.1.261) can be moved off it; the
	// subscription itself can't
	custom := i >= 0 && hasEndpoint(f.Providers[i])
	_, signedIn := find(Accounts(), from)
	sub := subscriptionID(from)
	if (signedIn || sub) && !custom {
		return fmt.Errorf("%s is a subscription: its id is its agent's", from)
	}
	switch {
	case to == "magpie":
		return errors.New(`"magpie" is what agents call the gateway itself; pick another id`)
	case to == strings.TrimSuffix(GroupPrefix, "/"):
		return errors.New(`"group" starts the ids of routing groups; pick another id`)
	case to == autoID:
		return errAutoID
	case subscriptionID(to):
		return fmt.Errorf("%q is the id of the %s subscription; pick another", to, to)
	}
	if i < 0 {
		return fmt.Errorf("no provider %q", from)
	}
	if slices.ContainsFunc(f.Providers, func(p Provider) bool { return p.ID == to }) {
		return fmt.Errorf("there is a provider %q already", to)
	}
	// an id another provider once had is this one's now; the calls
	// recorded under it until now stay that one's
	giveID(f.Providers, i, to, time.Now())
	p := &f.Providers[i]
	p.Was = slices.DeleteFunc(p.Was, func(w string) bool { return w == to })
	p.ID = to
	// the subscription's id is the subscription's: an old ref to it isn't
	// taken for this one
	if !sub && !slices.Contains(p.Was, from) {
		p.Was = append(p.Was, from)
	}
	for j := range f.Providers {
		for k, m := range f.Providers[j].Fallback {
			f.Providers[j].Fallback[k] = renamedRef(m, from, to)
		}
	}
	for j, id := range f.Order {
		if id == from {
			f.Order[j] = to
		}
	}
	for j := range f.Groups {
		g := &f.Groups[j]
		for k, m := range g.Members {
			g.Members[k] = renamedRef(m, from, to)
		}
		for k := range g.Rules {
			g.Rules[k].MapUses(func(u string) string { return renamedRef(u, from, to) })
		}
		for k, m := range g.Fast {
			g.Fast[k] = renamedRef(m, from, to)
		}
		for k, m := range g.Off {
			g.Off[k] = renamedRef(m, from, to)
		}
		g.Weights = renamedWeights(g.Weights, func(m string) string { return renamedRef(m, from, to) })
		g.Classifier = renamedRef(g.Classifier, from, to)
		g.Pick = renamedRef(g.Pick, from, to)
	}
	// the vendor's list last fetched goes with it
	os.Rename(catalog.LivePath(from), catalog.LivePath(to))
	os.Rename(catalog.LivePath(decisionsID(from)), catalog.LivePath(decisionsID(to)))
	if err := store(f); err != nil {
		return err
	}
	// so is its StepFun sign-in, kept by provider id
	if err := renameStepFun(from, to); err != nil {
		return err
	}
	s := settings.Load()
	changed := renameModelPrefs(&s, from, to)
	for a, names := range s.Visible {
		for k, n := range names {
			if strings.EqualFold(n, from) {
				s.Visible[a][k], changed = to, true
			}
		}
	}
	if changed {
		return settings.Save(s)
	}
	return nil
}

// renamedRef is a model id ("provider/model", "magpie/provider/model")
// with the provider from named to instead.
func renamedRef(ref, from, to string) string {
	pre, rest := "", ref
	if r, ok := strings.CutPrefix(ref, "magpie/"); ok {
		pre, rest = "magpie/", r
	}
	if m, ok := strings.CutPrefix(rest, from+"/"); ok {
		return pre + to + "/" + m
	}
	return ref
}

// RenamedRef is ref with the provider it names by an id the provider had
// named by the id it has now.
func RenamedRef(ref string) string {
	for old, now := range Renamed() {
		if r := renamedRef(ref, old, now); r != ref {
			return r
		}
	}
	return ref
}

// Renamed maps the ids providers had to the ones they have now. An id
// that a provider or a subscription has now is that one's, and is not
// in it.
func Renamed() map[string]string { return UsageRenames().now }

// Gave is an id a provider had and gave up to another (Provider.Gave).
type Gave struct {
	ID    string    `json:"id"`
	Until time.Time `json:"until"`
}

// Renames say which provider a call recorded under a provider id was
// made to, as its id is now: a provider renamed takes the calls recorded
// under the ids it had (Was), and an id another provider took since
// (Gave) is that one's only from then on (cybercaius on Discord: a
// renamed provider's usage was shown under its old id and its new one).
type Renames struct {
	now  map[string]string
	gave map[string][]gaveTo
}

type gaveTo struct {
	Until time.Time `json:"until"`
	To    string    `json:"to"`
}

// Of is the id the provider a call recorded under id at the time at has
// now; id itself when it was not renamed. A zero at is before every
// hand-over.
func (r Renames) Of(id string, at time.Time) string {
	for _, g := range r.gave[id] {
		if at.Before(g.Until) {
			return g.To
		}
	}
	if to, ok := r.now[id]; ok {
		return to
	}
	return id
}

// Timed is whether the id's calls go to more than one provider by when
// they were made, so Of needs each call's time.
func (r Renames) Timed(id string) bool { return len(r.gave[id]) > 0 }

// Each is every id a call recorded under id can go to (Of), by when it
// was made.
func (r Renames) Each(id string) []string {
	out := []string{r.Of(id, time.Time{})}
	for _, g := range r.gave[id] {
		out = append(out, g.To)
	}
	out = append(out, r.Of(id, time.Now().Add(100*365*24*time.Hour)))
	slices.Sort(out)
	return slices.Compact(out)
}

// RenamesOf are renames of ids to the ones they are now, none given away
// (for a test).
func RenamesOf(now map[string]string) Renames { return Renames{now: now} }

// Empty is whether no id is renamed.
func (r Renames) Empty() bool { return len(r.now) == 0 && len(r.gave) == 0 }

// Key is r as text, for a cache of what was worked out with it.
func (r Renames) Key() string {
	b, _ := json.Marshal([]any{r.now, r.gave})
	return string(b)
}

// lastRenames are the renames last read, kept for when providers.json
// can't be read: a failed read is not "nothing renamed", and the usage of
// every renamed provider would be shown split until it read again.
var lastRenames atomic.Pointer[Renames]

// UsageRenames are the renames of providers.json as it is now.
func UsageRenames() Renames {
	f, err := read()
	if err != nil {
		if r := lastRenames.Load(); r != nil {
			return *r
		}
		return Renames{now: map[string]string{}}
	}
	r := renamesOf(f.Providers)
	lastRenames.Store(&r)
	return r
}

func renamesOf(ps []Provider) Renames {
	r := Renames{now: map[string]string{}}
	held := map[string]bool{}
	for _, p := range ps {
		held[p.ID] = true
	}
	for _, p := range ps {
		for _, w := range p.Was {
			// the subscription's id is the subscription's (Rename)
			if !held[w] && !subscriptionID(w) {
				r.now[w] = p.ID
			}
		}
		for _, g := range p.Gave {
			if r.gave == nil {
				r.gave = map[string][]gaveTo{}
			}
			r.gave[g.ID] = append(r.gave[g.ID], gaveTo{g.Until, p.ID})
		}
	}
	for _, gs := range r.gave {
		slices.SortStableFunc(gs, func(a, b gaveTo) int { return a.Until.Compare(b.Until) })
	}
	return r
}

// giveID moves id out of the Was of every provider of ps but ps[taker]
// (-1 for one not among them) into its Gave at now: from now on id is
// another's, and what was recorded under it before stays the one's that
// had it.
func giveID(ps []Provider, taker int, id string, now time.Time) {
	for j := range ps {
		if j == taker || !slices.Contains(ps[j].Was, id) {
			continue
		}
		ps[j].Was = slices.DeleteFunc(ps[j].Was, func(w string) bool { return w == id })
		ps[j].Gave = append(ps[j].Gave, Gave{ID: id, Until: now})
	}
}

// OnAccountIDs are the user's own providers saved on a subscription's id,
// which hide that subscription once it is signed in (All lists the one
// saved): made before the id was a subscription's, as a WorkBuddy key was
// before WorkBuddy's plan was one. MoveOffAccountIDs moves them.
func OnAccountIDs() []string {
	var out []string
	for _, p := range load().Providers {
		if slices.Contains(accountIDs, p.ID) && hasEndpoint(p) {
			out = append(out, p.ID)
		}
	}
	return out
}

// FreeID is id, or id-2, id-3… whichever no provider or subscription has.
func FreeID(id string) string { return freeID(id) }

// hasEndpoint: a provider of the user's, not a subscription's model picks.
// Vertex AI's has none of the four: it is asked at its project's address.
func hasEndpoint(p Provider) bool {
	return p.Chat != "" || p.Responses != "" || p.Anthropic != "" || p.Gemini != "" || p.Decide != "" || p.IsVertex()
}
