package agent

import (
	"strings"

	"github.com/yetone/magpie/internal/provider"
)

// A group magpie found is listed as auto/<model> (provider.Group.Ref); it
// was group/auto-<slug>, and agents magpie set to it then still say so.
// The gateway serves that id as it did, but the lists magpie writes into
// an agent's files name the group by its new id only, so an agent that
// checks its model against its list (OpenCode) would find it gone. On its
// next write of the agent's lists (SyncCatalog), magpie moves each field
// it set to the old id onto the new one; a field the user set themselves,
// or changed since magpie set it, stays as they left it. So does a field
// magpie set to an auto/<model> spelt as the group is listed no longer
// (auto/glm-5-3-flash, now auto/glm-5.3-flash: the spelling follows
// models.dev's, Group.Spelled), which the gateway serves as ever.

// setToFoundGroups are the agents magpie set a field of to a found
// group's group/auto-<slug> or auto/<model>, by id: those
// renameFoundGroups looks at, so
// a sync reads no other agent's files for it. None while a provider of
// the user's is called auto, and auto/<model> is its.
func setToFoundGroups() map[string]bool {
	if !provider.AutoRefsOn() {
		return nil
	}
	appliedMu.Lock()
	defer appliedMu.Unlock()
	var out map[string]bool
	for id, ap := range appliedLoad() {
		for _, v := range ap.Fields {
			if strings.Contains(v, provider.GroupPrefix+"auto-") || strings.Contains(v, provider.AutoPrefix) {
				if out == nil {
					out = map[string]bool{}
				}
				out[id] = true
				break
			}
		}
	}
	return out
}

// renameFoundGroups moves a's fields that magpie set to a found group's
// group/auto-<slug>, or to an auto/<model> it is listed by no longer,
// onto the group's auto/<model> now, the agent's prefix and suffix kept. It answers whether it moved any.
func (a *Agent) renameFoundGroups() bool {
	if a.Native != nil || len(a.Fields) == 0 {
		return false
	}
	rec := appliedOf(a.ID).Fields
	if len(rec) == 0 {
		return false
	}
	moved := false
	vals := a.Values()
	for _, f := range a.Fields {
		v := vals[f.Key]
		// what magpie set, as it set it: a value of the user's own, or
		// one changed since, is theirs
		if want, ok := rec[f.Key]; !ok || v == "" || v != want || f.Set == nil {
			continue
		}
		now, ok := a.publicValue(v)
		if !ok {
			continue
		}
		if a.Apply(f.Key, now) == nil {
			moved = true
			vals = a.Values()
		}
	}
	return moved
}

// publicValue is v, an agent's value for a field, with a found group's
// group/auto-<slug>, or an auto/<model> spelt otherwise than the group is
// listed now, in it as the group's auto/<model>: its prefix (magpie/), an
// effort or [1m] after the id, and the agent's own suffix kept. ok is
// false when v names no such group, one magpie has no group for now, or
// the group as it is listed already.
func (a *Agent) publicValue(v string) (string, bool) {
	model, suffix, one := a.split(v)
	if !one {
		return v, false
	}
	i := strings.Index(model, provider.GroupPrefix+"auto-")
	if i < 0 {
		i = strings.Index(model, provider.AutoPrefix)
	}
	if i < 0 || i > 0 && model[i-1] != '/' {
		return v, false
	}
	ref := model[i:]
	pub := provider.PublicRef(ref)
	if pub == ref {
		return v, false
	}
	return model[:i] + pub + suffix, true
}
