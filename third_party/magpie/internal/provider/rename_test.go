package provider

import (
	"os"
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

func TestRename(t *testing.T) {
	isolate(t)
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, id := range []string{"relay", "other"} {
		if err := Save(Provider{ID: id, Name: "My " + id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1", "m2"}, Fallback: []string{"relay/m2"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SaveGroup(Group{ID: "top", Name: "Top", Members: []string{"relay/m1", "other/m1"},
		Rules: []Rule{{Use: "relay/m1", Intent: "code"}}, Classifier: "relay/m2"}); err != nil {
		t.Fatal(err)
	}
	s := settings.Load()
	s.Visible = map[string][]string{"codex": {"relay", "top"}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("relay", "http://127.0.0.1:1/v1", []catalog.Model{{ID: "m1"}}); err != nil {
		t.Fatal(err)
	}

	for _, bad := range []string{"other", "Not A Slug", "magpie", "group", "codex"} {
		if err := Rename("relay", bad); err == nil {
			t.Fatalf("renamed to %q", bad)
		}
	}
	if err := Rename("relay", "fast"); err != nil {
		t.Fatal(err)
	}
	p, err := Find("fast")
	if err != nil || p.Name != "My relay" || !slices.Equal(p.Was, []string{"relay"}) {
		t.Fatalf("%+v %v", p, err)
	}
	// the old id still reaches it, for an agent not yet told
	if q, err := Find("relay"); err != nil || q.ID != "fast" {
		t.Fatalf("%+v %v", q, err)
	}
	if q, m, ok := Resolve("relay/m1"); !ok || q.ID != "fast" || m != "m1" {
		t.Fatalf("%v %q %v", q.ID, m, ok)
	}
	if q, _ := Find("other"); !slices.Equal(q.Fallback, []string{"fast/m2"}) {
		t.Fatalf("fallback %v", q.Fallback)
	}
	g, _, _ := FindGroup("group/top")
	if !slices.Equal(g.Members, []string{"fast/m1", "other/m1"}) || g.Rules[0].Use != "fast/m1" || g.Classifier != "fast/m2" {
		t.Fatalf("%+v", g)
	}
	if v := settings.Load().Visible["codex"]; !slices.Equal(v, []string{"fast", "top"}) {
		t.Fatalf("visible %v", v)
	}
	if _, err := os.Stat(catalog.LivePath("fast")); err != nil {
		t.Fatal("the fetched list stayed behind")
	}
	if got := RenamedRef("magpie/relay/m1"); got != "magpie/fast/m1" {
		t.Fatal(got)
	}
	// a save that doesn't say keeps what it was
	p.Key = "k2"
	if err := Save(*p); err != nil {
		t.Fatal(err)
	}
	if p, _ := Find("fast"); !slices.Equal(p.Was, []string{"relay"}) {
		t.Fatalf("was %v", p.Was)
	}
	// its old id taken by another: that one is found by it
	if err := Rename("other", "relay"); err != nil {
		t.Fatal(err)
	}
	if q, _ := Find("relay"); q.Name != "My other" {
		t.Fatalf("%+v", q)
	}
	if p, _ := Find("fast"); len(p.Was) != 0 {
		t.Fatalf("was %v", p.Was)
	}
}
