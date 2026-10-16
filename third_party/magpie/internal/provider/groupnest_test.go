package provider

import (
	"strings"
	"testing"
)

// A group may have groups among its members, each routed as it says, but
// never one it is in: a loop is refused when saved, and cut if the file
// has one anyway.
func TestGroupsInGroups(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, id := range []string{"a", "b", "c"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m", "x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{Name: "Fast", Members: []string{"b/x", "c/x"}, Routing: Rotate}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Top", Members: []string{"a/x", "group/fast", "c/x"}, Routing: Ordered,
		Rules: []Rule{{Use: "group/fast", Tokens: 10}}}); err != nil {
		t.Fatal(err)
	}
	g, ms, ok := FindGroup("group/top")
	if !ok || len(g.Rules) != 1 {
		t.Fatalf("%v %+v", ok, g)
	}
	var got []string
	for _, m := range ms {
		got = append(got, m.ID+":"+strings.Join(m.Path, ">")+":"+strings.Join(m.Groups(), ","))
	}
	// c/x is fast's already: the group's own naming it again adds nothing
	if s := strings.Join(got, " "); s != "a/x:a/x: group/fast:group/fast>b/x:fast group/fast:group/fast>c/x:fast" {
		t.Fatal(s)
	}
	if b := ms[1].Below(1); b.ID != "b/x" || len(b.Via) != 0 || b.Provider.ID != "b" {
		t.Fatalf("%+v", b)
	}
	if p, m, ok := Resolve("group/top"); !ok || p.ID != "a" || m != "x" {
		t.Fatalf("resolve: %v %s %s", ok, p.ID, m)
	}

	for _, tc := range []struct {
		g   Group
		err string
	}{
		{Group{ID: "top", Name: "Top", Members: []string{"a/x", "group/top"}}, "can't be in itself"},
		{Group{ID: "fast", Name: "Fast", Members: []string{"b/x", "group/top"}}, "would be in itself"},
		{Group{Name: "New", Members: []string{"a/x", "group/nothing"}}, `no group "nothing"`},
	} {
		if err := SaveGroup(tc.g); err == nil || !strings.Contains(err.Error(), tc.err) {
			t.Errorf("%v: %v, want …%s", tc.g.Members, err, tc.err)
		}
	}
	if err := DeleteGroup("fast"); err == nil || !strings.Contains(err.Error(), "take it out first") {
		t.Fatalf("delete: %v", err)
	}

	// too deep: a chain of groups each in the next
	prev := "fast"
	var err error
	for i := 0; i < maxNest+1 && err == nil; i++ {
		name := "L" + string(rune('a'+i))
		err = SaveGroup(Group{Name: name, Members: []string{"group/" + prev}})
		prev = strings.ToLower(name)
	}
	if err == nil || !strings.Contains(err.Error(), "deep") {
		t.Fatalf("nesting: %v", err)
	}

	// a loop the file has anyway is cut where it comes round again
	all := []Group{
		{ID: "p", Members: []string{"group/q", "a/m"}},
		{ID: "q", Members: []string{"group/p", "b/m"}},
	}
	got = nil
	for _, m := range membersIn(providerEntries(), all, all[0]) {
		got = append(got, m.Provider.ID+"/"+m.Model)
	}
	if s := strings.Join(got, " "); s != "b/m a/m" {
		t.Fatal(s)
	}
}
