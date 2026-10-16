package provider

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/settings"
)

// A routing group picked for an agent shown only its picks stays picked
// when it is renamed (#2289, eachann1024: Pi's list lost its groups), and
// what other agents' lists say of it — taken out, put in an order, sent
// fast — moves to the new id too; nothing else of theirs changes.
func TestRenameGroupKeepsPicks(t *testing.T) {
	isolate(t)
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := Save(Provider{ID: "relay", Name: "Relay", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1", "m2"}}); err != nil {
		t.Fatal(err)
	}
	if err := SetAutoGroups(false); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"smart", "grok"} {
		if err := SaveGroup(Group{ID: id, Name: id, Members: []string{"relay/m1"}}); err != nil {
			t.Fatal(err)
		}
	}
	s := settings.Load()
	s.PickedModels = map[string][]string{"pi": {"group/smart", "relay/m1"}, "omp": {"relay/m2"}}
	s.HiddenModels = map[string][]string{"codex": {"group/smart", "relay/m2"}}
	s.OrderedModels = map[string][]string{"claude": {"relay/m2", "group/smart", "relay/m1"}}
	s.FastPicks = map[string][]string{"codex": {"group/smart"}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if shown, _ := CatalogFor("pi"); !slices.Contains(ids(shown), "group/smart") {
		t.Fatalf("not picked to begin with: %v", ids(shown))
	}
	if err := RenameGroup("smart", "zeta"); err != nil {
		t.Fatal(err)
	}
	shown, _ := CatalogFor("pi")
	if !slices.Equal(ids(shown), []string{"group/zeta", "relay/m1"}) {
		t.Fatalf("pi shown %v", ids(shown))
	}
	s = settings.Load()
	for name, got := range map[string][]string{
		"picked pi":  s.PickedModels["pi"],
		"picked omp": s.PickedModels["omp"],
		"hidden":     s.HiddenModels["codex"],
		"order":      s.OrderedModels["claude"],
		"fast":       s.FastPicks["codex"],
	} {
		want := map[string][]string{
			"picked pi":  {"group/zeta", "relay/m1"},
			"picked omp": {"relay/m2"},
			"hidden":     {"group/zeta", "relay/m2"},
			"order":      {"relay/m2", "group/zeta", "relay/m1"},
			"fast":       {"group/zeta"},
		}[name]
		if !slices.Equal(got, want) {
			t.Errorf("%s: %v, want %v", name, got, want)
		}
	}
	// the other group, not renamed, is still off in Pi's list
	if slices.Contains(ids(shown), "group/grok") {
		t.Fatal("an unpicked group came on")
	}
}
