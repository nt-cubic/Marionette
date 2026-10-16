package plugin

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

// Two plugins that sign in to one provider id (neiko on Discord: a
// third-party plugin and their own one, added by its folder). The host
// runs one of them for it; each says it signs in to the id, the other
// one says who serves it, the provider is listed once, and Prefer hands
// it to the other. Removing one leaves the other's untouched.
func TestTwoPluginsOneProvider(t *testing.T) {
	sandbox(t)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	src, err := os.ReadFile("testdata/fake/index.js")
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	mine, theirs := filepath.Join(dir, "mine", "index.js"), filepath.Join(dir, "theirs", "index.js")
	for _, f := range []string{mine, theirs} {
		if err := os.MkdirAll(filepath.Dir(f), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(f, src, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	served := func(want string) {
		t.Helper()
		ps, err := Providers(ctx)
		if err != nil {
			t.Fatal(err)
		}
		var specs []string
		for _, p := range ps {
			if p.ID == "fakeco" {
				specs = append(specs, p.Spec)
			}
		}
		if !reflect.DeepEqual(specs, []string{want}) {
			t.Fatalf("fakeco is served by %v, want just %s", specs, want)
		}
	}

	// the user's own first: it serves fakeco
	if _, err := Add(ctx, mine); err != nil {
		t.Fatal(err)
	}
	served(mine)
	// a third party's added after it serves it now, as in OpenCode, and
	// fakeco is listed once, not again as mine's from the list kept before
	if _, err := Add(ctx, theirs); err != nil {
		t.Fatal(err)
	}
	served(theirs)
	ls, err := Plugins(ctx)
	if err != nil || len(ls) != 2 {
		t.Fatalf("Plugins = %+v, %v", ls, err)
	}
	by := map[string]Loaded{}
	for _, l := range ls {
		by[l.Spec] = l
	}
	if l := by[mine]; !reflect.DeepEqual(l.Provides, []string{"fakeco"}) || !reflect.DeepEqual(l.ServedBy, map[string]string{"fakeco": theirs}) {
		t.Fatalf("mine loaded as %+v: it signs in to fakeco, which theirs serves", l)
	}
	if l := by[theirs]; !reflect.DeepEqual(l.Provides, []string{"fakeco"}) || l.ServedBy != nil {
		t.Fatalf("theirs loaded as %+v: it signs in to fakeco and serves it", l)
	}
	want := map[string][]Clash{
		mine:   {{ID: "fakeco", By: theirs, With: []string{theirs}}},
		theirs: {{ID: "fakeco", By: theirs, With: []string{mine}}},
	}
	if c := Clashes(ls); !reflect.DeepEqual(c, want) {
		t.Fatalf("Clashes = %+v, want %+v", c, want)
	}

	// picked for it, mine serves it again; theirs stays installed
	if err := Prefer(mine, "fakeco"); err != nil {
		t.Fatal(err)
	}
	served(mine)
	if ls, err = Plugins(ctx); err != nil {
		t.Fatal(err)
	}
	if c := Clashes(ls); len(c[mine]) != 1 || c[mine][0].By != mine || c[theirs][0].By != mine {
		t.Fatalf("after Prefer, Clashes = %+v", c)
	}
	// ...and stays picked after a restart
	Restart()
	served(mine)

	// switched off, it leaves fakeco to theirs; on again, it has it back
	if err := SetOff(mine, true); err != nil {
		t.Fatal(err)
	}
	served(theirs)
	if err := SetOff(mine, false); err != nil {
		t.Fatal(err)
	}
	served(mine)

	// removed, its pick goes with it, and theirs serves fakeco
	if err := Remove(ctx, mine); err != nil {
		t.Fatal(err)
	}
	if p := Load().Prefer; len(p) != 0 {
		t.Fatalf("after removing mine, Prefer = %v", p)
	}
	served(theirs)
}

func TestClashes(t *testing.T) {
	ls := []Loaded{
		{Spec: "a", Provides: []string{"x", "y"}, ServedBy: map[string]string{"x": "c"}},
		{Spec: "b", Error: "broken"},
		{Spec: "c", Provides: []string{"x"}},
		{Spec: "d", Provides: []string{"z"}},
	}
	want := map[string][]Clash{
		"a": {{ID: "x", By: "c", With: []string{"c"}}},
		"c": {{ID: "x", By: "c", With: []string{"a"}}},
	}
	if c := Clashes(ls); !reflect.DeepEqual(c, want) {
		t.Fatalf("Clashes = %+v, want %+v", c, want)
	}
	if c := Clashes(nil); len(c) != 0 {
		t.Fatalf("Clashes(nil) = %+v", c)
	}
}

// A name is the plugin added as it exactly before one whose package has
// that name: switching one off, setting its options or picking it never
// reaches another.
func TestPluginNamedExactlyFirst(t *testing.T) {
	storeSandbox(t)
	writeList(t, `{"plugins":[{"spec":"acme@latest"},{"spec":"acme"}]}`)
	if err := SetOff("acme", true); err != nil {
		t.Fatal(err)
	}
	if ps := Load().Plugins; ps[0].Off || !ps[1].Off {
		t.Fatalf("SetOff(acme) = %+v, want acme off, not acme@latest", ps)
	}
	if err := SetOptions("acme", map[string]any{"k": 1.0}); err != nil {
		t.Fatal(err)
	}
	if ps := Load().Plugins; ps[0].Options != nil || ps[1].Options == nil {
		t.Fatalf("SetOptions(acme) = %+v", ps)
	}
	if err := Prefer("acme", "acme"); err != nil {
		t.Fatal(err)
	}
	if p := Load().Prefer; !reflect.DeepEqual(p, map[string]string{"acme": "acme"}) {
		t.Fatalf("Prefer = %v", p)
	}
	if err := Prefer("acme@latest", "acme"); err != nil {
		t.Fatal(err)
	}
	if p := Load().Prefer; !reflect.DeepEqual(p, map[string]string{"acme": "acme@latest"}) {
		t.Fatalf("Prefer = %v", p)
	}
	if err := Prefer("nosuch", "acme"); err == nil {
		t.Fatal("Prefer took a plugin that isn't installed")
	}
	if err := Prefer("acme", ""); err == nil {
		t.Fatal("Prefer took no provider")
	}
}

// A provider kept from before for a plugin that told none this time isn't
// kept when another plugin serves its id now: it would be listed twice.
func TestKeepUnloadedLeavesAServedID(t *testing.T) {
	storeSandbox(t)
	writeList(t, `{"plugins":[{"spec":"/me/acme"},{"spec":"acme-auth"},{"spec":"/me/broken"}]}`)
	now := []Provider{{ID: "acme", Spec: "acme-auth"}}
	last := []Provider{{ID: "acme", Spec: "/me/acme"}, {ID: "other", Spec: "/me/broken"}}
	got := keepUnloaded(now, last)
	want := []Provider{{ID: "acme", Spec: "acme-auth"}, {ID: "other", Spec: "/me/broken"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("keepUnloaded = %+v, want %+v", got, want)
	}
}
