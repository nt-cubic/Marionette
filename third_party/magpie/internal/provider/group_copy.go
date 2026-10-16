package provider

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"
)

// CopyGroup makes a group of the user's that is the group id as it is now
// (lc on Discord): its models and patterns, routing, rules, classifier,
// effort, context, levels, fast and off members, under name — "<name>
// copy" when empty, numbered past a name or id in use — listed right after
// the one it copies. A found group's copy is the user's, with the members
// it has now. The copy is on, whether the group was or not, and the
// original is left as it is.
func CopyGroup(id, name string) (Group, error) {
	all := Groups()
	i := slices.IndexFunc(all, func(g Group) bool { return g.ID == id && !g.Hidden })
	if i < 0 {
		return Group{}, fmt.Errorf("no group %q", id)
	}
	src := all[i]
	// a copy shares nothing with the group it was read from: the rules'
	// time windows are pointers, the lists slices
	raw, err := json.Marshal(src)
	if err != nil {
		return Group{}, err
	}
	var g Group
	if err := json.Unmarshal(raw, &g); err != nil {
		return Group{}, err
	}
	g.Auto, g.Hidden, g.Disabled = false, false, false

	name = strings.TrimSpace(name)
	if name == "" {
		name = src.Name + " copy"
	}
	g.Name, g.ID = freeGroupName(all, name, "")
	if err := SaveGroup(g); err != nil {
		return Group{}, err
	}

	// listed after the one it copies, not at the end of the user's
	order := make([]string, 0, len(all)+1)
	for _, o := range all {
		order = append(order, o.ID)
		if o.ID == src.ID {
			order = append(order, g.ID)
		}
	}
	if err := SetGroupOrder(order); err != nil {
		return Group{}, err
	}
	for _, o := range Groups() {
		if o.ID == g.ID {
			return o, nil
		}
	}
	return g, nil
}

// freeGroupName is name, numbered past a name in use ("name 2"), and slug —
// or, when it is "", the id made of the name — numbered past an id in use or a group removed ("slug-2"):
// what a group made in a click (a copy, a template) is saved as, so it
// never replaces one there is.
func freeGroupName(all []Group, name, slug string) (string, string) {
	names, ids := map[string]bool{}, map[string]bool{}
	for _, o := range all {
		ids[o.ID] = true
		if !o.Hidden {
			names[strings.ToLower(o.Name)] = true
		}
	}
	for _, removed := range RemovedGroups() {
		ids[removed] = true
	}
	base := name
	for n := 2; names[strings.ToLower(name)]; n++ {
		name = fmt.Sprintf("%s %d", base, n)
	}
	if slug == "" {
		slug = GroupSlug(name)
	}
	if slug == "" {
		slug = "group"
	}
	id := slug
	for n := 2; ids[id]; n++ {
		id = fmt.Sprintf("%s-%d", slug, n)
	}
	return name, id
}
