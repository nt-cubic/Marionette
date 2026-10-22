package provider

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// autoRefHome is a sandbox HOME with two providers serving Claude Opus 5.5,
// each spelling it its own way, and a models.dev catalog listing it as
// Anthropic spells it, with its window and reply limit.
func autoRefHome(t *testing.T) {
	t.Helper()
	isolate(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := os.MkdirAll(filepath.Dir(catalog.CachePath()), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.CachePath(), []byte(`{"anthropic": {"models": {
	  "claude-opus-5.5": {"id":"claude-opus-5.5","name":"Claude Opus 5.5","limit":{"context":400000,"output":96000}}}}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	for id, m := range map[string]string{"a": "claude-opus-5-5", "b": "claude-opus-5.5"} {
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{m, "x"}}); err != nil {
			t.Fatal(err)
		}
	}
}

// A group magpie found is listed as auto/<model>, the model's id as a
// vendor spells it (ZeroClover on X), so a client looking the part after
// the slash up on models.dev finds the model; its old group/auto-<slug> is
// the same group, and a group the user made stays group/<id>.
func TestFoundGroupListedByItsModelsID(t *testing.T) {
	autoRefHome(t)
	if err := SaveGroup(Group{Name: "Mine", Members: []string{"a/x", "b/x"}}); err != nil {
		t.Fatal(err)
	}
	refs := map[string]string{}
	for _, g := range Groups() {
		refs[g.ID] = g.Ref()
	}
	if refs["auto-claude-opus-5-5"] != "auto/claude-opus-5.5" {
		t.Errorf("the found group's id: %q", refs["auto-claude-opus-5-5"])
	}
	if refs["mine"] != "group/mine" {
		t.Errorf("the user's group's id: %q", refs["mine"])
	}
	var e Entry
	for _, c := range Catalog() {
		if c.Group == "auto-claude-opus-5-5" {
			e = c
		}
	}
	if e.ID != "auto/claude-opus-5.5" {
		t.Fatalf("the catalog lists the group as %q", e.ID)
	}
	// what a client does with it: the model's id after auto/, on models.dev
	model := strings.TrimPrefix(e.ID, AutoPrefix)
	if catalog.ContextOf(model) != 400000 || catalog.OutputOf(model) != 96000 {
		t.Errorf("models.dev has %s at %d/%d", model, catalog.ContextOf(model), catalog.OutputOf(model))
	}
	if e.Context != 400000 || e.Output != 96000 {
		t.Errorf("the group's own limits: %d/%d", e.Context, e.Output)
	}

	for in, want := range map[string]string{
		"auto/claude-opus-5.5":           "group/auto-claude-opus-5-5",
		"auto/anthropic/Claude-Opus-5.5": "group/auto-claude-opus-5-5",
		"auto/claude-opus-5-5":           "group/auto-claude-opus-5-5",
		"auto/claude-opus-5.5:high[1m]":  "group/auto-claude-opus-5-5:high[1m]",
		"group/auto-claude-opus-5-5":     "group/auto-claude-opus-5-5",
		"a/claude-opus-5-5":              "a/claude-opus-5-5",
		"auto/":                          "auto/",
		"openrouter/auto":                "openrouter/auto",
	} {
		if got := CanonicalRef(in); got != want {
			t.Errorf("CanonicalRef(%q) = %q, want %q", in, got, want)
		}
	}
	for in, want := range map[string]string{
		"group/auto-claude-opus-5-5":          "auto/claude-opus-5.5",
		"group/auto-claude-opus-5-5:high[1m]": "auto/claude-opus-5.5:high[1m]",
		"group/mine":                          "group/mine",
		"group/auto-gone":                     "group/auto-gone",
	} {
		if got := PublicRef(in); got != want {
			t.Errorf("PublicRef(%q) = %q, want %q", in, got, want)
		}
	}

	// both ids are served, as the one group
	for _, id := range []string{"auto/claude-opus-5.5", "group/auto-claude-opus-5-5", "auto/claude-opus-5.5[1m]"} {
		p, m, ok := Resolve(id)
		if !ok || p.ID != e.Provider.ID || m != e.Model {
			t.Errorf("Resolve(%q) = %q %q %v, want %q %q", id, p.ID, m, ok, e.Provider.ID, e.Model)
		}
		if gid, ok := GroupIDOf(id); !ok || gid != "auto-claude-opus-5-5" {
			t.Errorf("GroupIDOf(%q) = %q %v", id, gid, ok)
		}
	}

	// a group picked as a member by its new id is kept by its old one
	if err := SaveGroup(Group{ID: "mine", Name: "Mine", Members: []string{"a/x", "auto/claude-opus-5.5"}}); err != nil {
		t.Fatal(err)
	}
	for _, g := range Groups() {
		if g.ID == "mine" && !slices.Equal(g.Members, []string{"a/x", "group/auto-claude-opus-5-5"}) {
			t.Errorf("stored members: %v", g.Members)
		}
	}
}

// "auto" is no new provider's id: auto/<model> names found groups. One
// the user saved before keeps its models: while it is there, auto/<x> is
// its model x, and found groups are listed by their old ids.
func TestProviderNamedAuto(t *testing.T) {
	autoRefHome(t)
	if err := Save(Provider{ID: "auto", Name: "Auto", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"y"}}); !errors.Is(err, errAutoID) {
		t.Fatalf("a new provider named auto: %v", err)
	}
	if err := Rename("a", "auto"); !errors.Is(err, errAutoID) {
		t.Fatalf("renaming a provider to auto: %v", err)
	}
	if PluginID("auto") != "auto-plugin" || PluginID("group") != "group-plugin" {
		t.Errorf("plugin ids: %q %q", PluginID("auto"), PluginID("group"))
	}

	// one in providers.json from before
	raw, err := os.ReadFile(Path())
	if err != nil {
		t.Fatal(err)
	}
	s := strings.Replace(string(raw), `"providers": [`, `"providers": [{"id": "auto", "name": "Auto", "key": "k", "chat": "http://127.0.0.1:1/v1", "models": ["claude-opus-5.5", "y"]},`, 1)
	if s == string(raw) {
		t.Fatalf("providers.json: %s", raw)
	}
	if err := os.WriteFile(Path(), []byte(s), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := Find("auto"); err != nil {
		t.Fatal(err)
	}
	if AutoRefsOn() {
		t.Fatal("auto/<model> still names found groups beside a provider called auto")
	}
	for _, id := range []string{"auto/y", "auto/claude-opus-5.5"} {
		p, m, ok := Resolve(id)
		if !ok || p.ID != "auto" || m != strings.TrimPrefix(id, "auto/") {
			t.Errorf("Resolve(%q) = %q %q %v: not the auto provider's model", id, p.ID, m, ok)
		}
		if CanonicalRef(id) != id {
			t.Errorf("CanonicalRef(%q) = %q", id, CanonicalRef(id))
		}
	}
	var listed []string
	for _, e := range Catalog() {
		if e.Group != "" {
			listed = append(listed, e.ID)
		}
	}
	if !slices.Contains(listed, "group/auto-claude-opus-5-5") || slices.ContainsFunc(listed, func(id string) bool { return strings.HasPrefix(id, AutoPrefix) }) {
		t.Errorf("groups listed beside the auto provider: %v", listed)
	}
	// still the user's to save
	p, _ := Find("auto")
	p.Name = "Mine"
	if err := Save(*p); err != nil {
		t.Errorf("saving the stored auto provider: %v", err)
	}
}

// A pick made one model at a time in an agent's lists, saved while a found
// group was listed as group/auto-<slug>, holds for it by auto/<model>: a
// group taken out stays out, one picked stays shown, and the order kept.
func TestFoundGroupPicksSavedByTheOldID(t *testing.T) {
	autoRefHome(t)
	old, now := "group/auto-claude-opus-5-5", "auto/claude-opus-5.5"
	s := settings.Load()
	s.HiddenModels = map[string][]string{"pi": {old}}
	s.PickedModels = map[string][]string{"opencode": {old}}
	s.OrderedModels = map[string][]string{"pi": {old, "a/x"}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if !ModelOff("pi")(now) {
		t.Errorf("hidden as %s, listed as %s again", old, now)
	}
	if ModelOff("opencode")(now) {
		t.Errorf("picked as %s, left out as %s", old, now)
	}
	if got := ModelOrder("pi"); !slices.Equal(got, []string{now, "a/x"}) {
		t.Errorf("order: %v", got)
	}
}
