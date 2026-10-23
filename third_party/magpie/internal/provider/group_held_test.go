package provider

import (
	"fmt"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// groupHeldFixture is a dozen providers serving the same models, so
// magpie finds a group of each, and a group of the user's of four.
func groupHeldFixture(t *testing.T) {
	t.Helper()
	isolate(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("MAGPIE_NO_ACCOUNTS", "1")
	var models []string
	for i := range 10 {
		models = append(models, fmt.Sprintf("m%d", i))
	}
	for i := range 12 {
		if err := Save(Provider{ID: fmt.Sprintf("p%d", i), Name: fmt.Sprintf("P%d", i), Key: "k", Models: models, Anthropic: "http://127.0.0.1:9"}); err != nil {
			t.Fatal(err)
		}
	}
	if err := settings.Save(settings.Settings{DisabledModels: []string{"p11/m9"}}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{ID: "g4", Name: "G4", Members: []string{"p0/m1", "p1/m1", "p2/m1", "p3/m1"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	Changed()
}

// A request to a group asks for the catalog's groups several times —
// Resolve's Catalog, and Served for the members' images, the reply's
// length and the window — and each built every group's entry again, from
// every member of every group: the burst of CPU at once that a request to
// a group cost with many providers (colmugx and Yayoi_no_yume on X). While
// a request holds the catalog they are built once.
func TestGroupEntriesBuiltOnceWhileHeld(t *testing.T) {
	groupHeldFixture(t)
	release := Hold()
	defer release()
	entries := providerEntries()
	first := groupEntries(entries)
	if len(first) < 2 {
		t.Fatalf("the fixture has %d groups; the test needs the found ones too", len(first))
	}
	if again := groupEntries(entries); &again[0] != &first[0] {
		t.Fatal("the groups' entries were built again under one Hold")
	}
	// what a caller is given is its own to change
	c := Catalog()
	c[0].Name = "changed"
	if Catalog()[0].Name == "changed" || Served()[0].Name == "changed" {
		t.Fatal("a change to Catalog's entries reached the next caller's")
	}
	// another catalog than the one held is built from, not answered for
	other := append([]Entry(nil), entries[:len(entries)/2]...)
	if got := groupEntries(other); len(got) > 0 && &got[0] == &first[0] {
		t.Fatal("the groups of another catalog were the held one's")
	}
	// a write drops them
	if err := SaveGroup(Group{ID: "g4", Name: "Renamed", Members: []string{"p0/m1", "p1/m1"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	found := false
	for _, e := range Catalog() {
		found = found || e.ID == GroupPrefix+"g4" && e.Name == "Renamed"
	}
	if !found {
		t.Fatal("a group saved under the Hold kept its old entry")
	}
}

// Each group's members (membersIn) asked whether the user disabled them,
// and that parsed settings.json each time: a catalog of found groups
// parsed it once a group, on each of the several looks a request makes.
func TestGroupLooksParseNoSettingsPerGroupWhileHeld(t *testing.T) {
	groupHeldFixture(t)
	release := Hold()
	defer release()
	Served() // the first look builds what is held
	const looks = 50
	before := settings.Parses()
	for range looks {
		Served()
		Groups()
		if _, _, ok := FindGroup(GroupPrefix + "g4"); !ok {
			t.Fatal("no group g4")
		}
	}
	if n := settings.Parses() - before; n >= looks {
		t.Fatalf("settings.json parsed %d times in %d looks at the groups under one Hold", n, looks)
	}
	// and a model disabled is still left out
	for _, e := range Served() {
		if e.ID == "p11/m9" {
			t.Fatal("a disabled model was served")
		}
	}
}

// BenchmarkGroupEntries is the groups' entries of a catalog of 30
// providers serving the same 60 models (60 groups magpie finds, of 30
// members each), built afresh: what each request built several times.
func BenchmarkGroupEntries(b *testing.B) {
	isolate(b)
	home := b.TempDir()
	b.Setenv("HOME", home)
	b.Setenv("USERPROFILE", home)
	b.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	b.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	b.Setenv("MAGPIE_NO_ACCOUNTS", "1")
	var models []string
	for i := range 60 {
		models = append(models, fmt.Sprintf("m%d", i))
	}
	for i := range 30 {
		if err := Save(Provider{ID: fmt.Sprintf("p%d", i), Name: fmt.Sprintf("P%d", i), Key: "k", Models: models, Anthropic: "http://127.0.0.1:9"}); err != nil {
			b.Fatal(err)
		}
	}
	Changed()
	entries := providerEntries()
	b.ResetTimer()
	for range b.N {
		buildGroupEntries(entries)
	}
}

// A request to a group looked its group up through FindGroup and the
// other group readers, and each built every group again: reading
// providers.json's groups, matching each pattern against every model and
// finding the automatic groups over the catalog. While a request holds the
// catalog the groups are built once, and each caller gets its own copy.
func TestGroupsBuiltOnceWhileHeld(t *testing.T) {
	groupHeldFixture(t)
	release := Hold()
	defer release()
	Groups()
	before := groupsBuilt.Load()
	for range 20 {
		Groups()
		if _, _, ok := FindGroup(GroupPrefix + "g4"); !ok {
			t.Fatal("no group g4")
		}
	}
	if n := groupsBuilt.Load() - before; n != 0 {
		t.Fatalf("the groups were built %d times more in 40 looks under one Hold, want 0", n)
	}
	// a caller's change to its copy is not the next caller's
	var g4 Group
	for _, g := range Groups() {
		if g.ID == "g4" {
			g4 = g
		}
	}
	g4.Members[0] = "changed"
	for _, g := range Groups() {
		if g.ID == "g4" && g.Members[0] == "changed" {
			t.Fatal("a change to a held group's members reached the next caller")
		}
	}
	// a write builds them again
	if err := SaveGroup(Group{ID: "g4", Name: "Renamed", Members: []string{"p0/m1", "p1/m1"}, Routing: Ordered}); err != nil {
		t.Fatal(err)
	}
	if g, _, ok := FindGroup(GroupPrefix + "g4"); !ok || g.Name != "Renamed" {
		t.Fatalf("a group saved under the Hold kept its old self: %+v", g)
	}
}

// cloneGroups shares nothing with what it copies: every slice, map and
// pointer of a Group and of its Rules, found by reflection, so a field
// added later is copied too or this fails.
func TestCloneGroupsSharesNothing(t *testing.T) {
	var g Group
	fillEvery(reflect.ValueOf(&g).Elem())
	if len(g.Rules) == 0 {
		t.Fatal("the fill made no rule")
	}
	c := cloneGroups([]Group{g})[0]
	if !reflect.DeepEqual(c, g) {
		t.Fatalf("the copy differs:\n%+v\n%+v", c, g)
	}
	if path := sharedRef(reflect.ValueOf(g), reflect.ValueOf(c), "Group"); path != "" {
		t.Fatalf("%s is shared with the held group", path)
	}
}

// fillEvery gives every field of v a value, and each slice, map and pointer one
// element.
func fillEvery(v reflect.Value) {
	switch v.Kind() {
	case reflect.Struct:
		for i := range v.NumField() {
			if v.Type().Field(i).IsExported() {
				fillEvery(v.Field(i))
			}
		}
	case reflect.Slice:
		v.Set(reflect.MakeSlice(v.Type(), 1, 1))
		fillEvery(v.Index(0))
	case reflect.Map:
		v.Set(reflect.MakeMap(v.Type()))
		k, e := reflect.New(v.Type().Key()).Elem(), reflect.New(v.Type().Elem()).Elem()
		fillEvery(k)
		fillEvery(e)
		v.SetMapIndex(k, e)
	case reflect.Pointer:
		v.Set(reflect.New(v.Type().Elem()))
		fillEvery(v.Elem())
	case reflect.String:
		v.SetString("x")
	case reflect.Bool:
		v.SetBool(true)
	case reflect.Int, reflect.Int64, reflect.Int32:
		v.SetInt(1)
	case reflect.Float64:
		v.SetFloat(1)
	}
}

// sharedRef names the first slice, map or pointer a and b hold in common.
func sharedRef(a, b reflect.Value, path string) string {
	switch a.Kind() {
	case reflect.Struct:
		for i := range a.NumField() {
			if a.Type().Field(i).IsExported() {
				if p := sharedRef(a.Field(i), b.Field(i), path+"."+a.Type().Field(i).Name); p != "" {
					return p
				}
			}
		}
	case reflect.Slice:
		if a.Len() > 0 && a.Pointer() == b.Pointer() {
			return path
		}
		for i := range a.Len() {
			if p := sharedRef(a.Index(i), b.Index(i), fmt.Sprintf("%s[%d]", path, i)); p != "" {
				return p
			}
		}
	case reflect.Map, reflect.Pointer:
		if !a.IsNil() && a.Pointer() == b.Pointer() {
			return path
		}
		if a.Kind() == reflect.Pointer && !a.IsNil() {
			return sharedRef(a.Elem(), b.Elem(), path)
		}
	}
	return ""
}

// BenchmarkGroupLooksHeld is the group look-ups one request to a group
// makes under its Hold (FindGroup for the route, its members and the
// window, and the group list), over 30 providers serving the same 60
// models.
func BenchmarkGroupLooksHeld(b *testing.B) {
	isolate(b)
	home := b.TempDir()
	b.Setenv("HOME", home)
	b.Setenv("USERPROFILE", home)
	b.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	b.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	b.Setenv("MAGPIE_NO_ACCOUNTS", "1")
	var models []string
	for i := range 60 {
		models = append(models, fmt.Sprintf("m%d", i))
	}
	for i := range 30 {
		if err := Save(Provider{ID: fmt.Sprintf("p%d", i), Name: fmt.Sprintf("P%d", i), Key: "k", Models: models, Anthropic: "http://127.0.0.1:9"}); err != nil {
			b.Fatal(err)
		}
	}
	if err := SaveGroup(Group{ID: "g4", Name: "G4", Members: []string{"p0/m1", "p1/m1", "p2/m1", "p3/m1"}, Routing: Ordered}); err != nil {
		b.Fatal(err)
	}
	Changed()
	defer func(d time.Duration) { heldFor = d }(heldFor)
	heldFor = time.Hour // the look-ups of one request, not the Hold's age on a loaded machine
	release := Hold()
	defer release()
	providerEntries()
	b.ResetTimer()
	for range b.N {
		for range 4 {
			if _, _, ok := FindGroup(GroupPrefix + "g4"); !ok {
				b.Fatal("no group g4")
			}
		}
		Groups()
	}
}
