package provider

import (
	"strings"
	"testing"
)

// A Manual group switched from the tray panel (#2489) sends every request
// to the member picked, from the next request on, and keeps the rest of
// itself; a group that doesn't route by hand, a model not in it, or a
// group magpie hasn't are refused and change nothing.
func TestPickGroup(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	for _, id := range []string{"a", "b"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{Name: "Hand", Members: []string{"a/x", "b/x"}, Routing: Manual, Patience: 45}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Order", Members: []string{"a/x", "b/x"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	routes := func(id string) string {
		t.Helper()
		_, ms, ok := FindGroup("group/" + id)
		if !ok {
			return "-"
		}
		var out []string
		for _, m := range ms {
			out = append(out, m.Provider.ID+"/"+m.Model)
		}
		return strings.Join(out, " ")
	}
	if s := routes("hand"); s != "a/x" {
		t.Fatalf("before: %s", s)
	}
	if err := PickGroup("hand", "b/x"); err != nil {
		t.Fatal(err)
	}
	if s := routes("hand"); s != "b/x" {
		t.Fatalf("picked b/x, routes to %s", s)
	}
	g := mustGroup(t, "hand")
	if g.Routing != Manual || g.Name != "Hand" || len(g.Members) != 2 || g.Patience != 45 {
		t.Fatalf("the rest of the group changed: %+v", g)
	}
	for _, c := range []struct{ id, pick, want string }{
		{"order", "b/x", "Manual"},
		{"hand", "c/x", "not in"},
		{"nope", "a/x", "no group"},
	} {
		if err := PickGroup(c.id, c.pick); err == nil || !strings.Contains(err.Error(), c.want) {
			t.Errorf("PickGroup(%s, %s) = %v, want %q", c.id, c.pick, err, c.want)
		}
	}
	if s := routes("hand"); s != "b/x" {
		t.Fatalf("a refused pick moved it: %s", s)
	}
	if s := routes("order"); s != "a/x b/x" {
		t.Fatalf("a refused pick changed the ordered group: %s", s)
	}
}

func mustGroup(t *testing.T, id string) Group {
	t.Helper()
	f, err := read()
	if err != nil {
		t.Fatal(err)
	}
	for _, g := range f.Groups {
		if g.ID == id {
			return g
		}
	}
	t.Fatalf("no group %s", id)
	return Group{}
}
