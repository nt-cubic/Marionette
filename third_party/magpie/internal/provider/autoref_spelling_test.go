package provider

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// glmCatalog lists GLM-5.3 Flash as models.dev does: glm-5.3-flash under
// most providers, named GLM-5.3-Flash, and Volcengine Ark's dashed
// glm-5-3-flash under one, named otherwise.
const glmCatalog = `{
	  "zai": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}},
	  "fireworks": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}},
	  "openrouter": {"models": {"z-ai/glm-5.3-flash": {"id":"z-ai/glm-5.3-flash","name":"GLM-5.3-Flash"}}},
	  "volcengine": {"models": {"glm-5-3-flash": {"id":"glm-5-3-flash","name":"GLM 5.3 Flash"}}}}`

// glmDottedOnly lists it under the dotted spelling alone.
const glmDottedOnly = `{
	  "zai": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}},
	  "fireworks": {"models": {"glm-5.3-flash": {"id":"glm-5.3-flash","name":"GLM-5.3-Flash"}}}}`

// spellingHome is a sandbox HOME with the models.dev catalog cat, and
// models the providers' own, one provider each, in the order they are
// added.
func spellingHome(t *testing.T, cat string, models ...string) {
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
	if err := os.WriteFile(catalog.CachePath(), []byte(cat), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	for i, m := range models {
		id := string(rune('a' + i))
		if err := Save(Provider{ID: id, Name: id, Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{m}}); err != nil {
			t.Fatal(err)
		}
	}
}

func foundGroup(t *testing.T, id string) Group {
	t.Helper()
	for _, g := range Groups() {
		if g.ID == id {
			return g
		}
	}
	t.Fatalf("no group %s in %+v", id, Groups())
	return Group{}
}

// A found group's id and its name come from one choice (XYenon on
// Discord): the spelling models.dev gives the model most, and the name it
// gives that spelling, whichever provider was added first and however it
// spells the model. They were picked apart: the name was the first
// provider's id (glm-5-3-flash) beside auto/glm-5.3-flash, and the id
// itself followed the providers' order. Every spelling it was listed by
// before is still the group, and a name the user gave it stays theirs.
func TestFoundGroupIDAndNameAreOneSpelling(t *testing.T) {
	for _, tc := range []struct {
		name   string
		cat    string
		models []string
	}{
		// the reporter's: the first provider's spelling, which models.dev
		// doesn't list, was the name, and the one it lists the id
		{"a spelling models.dev lacks first", glmDottedOnly, []string{"glm-5-3-flash", "glm-5.3-flash"}},
		{"Ark's dashed spelling first", glmCatalog, []string{"glm-5-3-flash", "glm-5.3-flash"}},
		{"Ark's dated id first", glmCatalog, []string{"glm-5-3-flash-260828", "glm-5.3-flash"}},
		{"the dotted spelling first", glmCatalog, []string{"glm-5.3-flash", "glm-5-3-flash"}},
		{"no provider serves models.dev's spelling", glmCatalog, []string{"glm-5-3-flash", "GLM-5.3-Flash-260828"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			models := tc.models
			spellingHome(t, tc.cat, models...)
			g := foundGroup(t, "auto-glm-5-3-flash")
			if g.Ref() != "auto/glm-5.3-flash" || g.Name != "GLM-5.3-Flash" {
				t.Fatalf("the group is %q named %q, want auto/glm-5.3-flash named GLM-5.3-Flash", g.Ref(), g.Name)
			}
			// what agents may hold from before: each still the group
			for _, old := range []string{"auto/glm-5-3-flash", "auto/glm-5.3-flash", "group/auto-glm-5-3-flash"} {
				if gid, ok := GroupIDOf(old); !ok || gid != "auto-glm-5-3-flash" {
					t.Errorf("GroupIDOf(%q) = %q %v", old, gid, ok)
				}
				if _, _, ok := Resolve(old); !ok {
					t.Errorf("%s isn't served", old)
				}
			}
			for in, want := range map[string]string{
				"auto/glm-5-3-flash":       "auto/glm-5.3-flash",
				"auto/glm-5-3-flash:high":  "auto/glm-5.3-flash:high",
				"auto/glm-5-3-flash[1m]":   "auto/glm-5.3-flash[1m]",
				"group/auto-glm-5-3-flash": "auto/glm-5.3-flash",
				"auto/glm-5.3-flash":       "auto/glm-5.3-flash",
				"auto/no-such-model":       "auto/no-such-model",
				"a/" + models[0]:           "a/" + models[0],
			} {
				if got := PublicRef(in); got != want {
					t.Errorf("PublicRef(%q) = %q, want %q", in, got, want)
				}
			}
		})
	}

	t.Run("the user's name", func(t *testing.T) {
		spellingHome(t, glmCatalog, "glm-5-3-flash", "glm-5.3-flash")
		g := foundGroup(t, "auto-glm-5-3-flash")
		g.Name = "Flash"
		if err := SaveGroup(g); err != nil {
			t.Fatal(err)
		}
		g = foundGroup(t, "auto-glm-5-3-flash")
		if g.Name != "Flash" || g.Ref() != "auto/glm-5.3-flash" {
			t.Errorf("the user's group is %q named %q", g.Ref(), g.Name)
		}
	})
}

// A model models.dev doesn't know keeps the spelling the members give it,
// and its name is that id, not another provider's spelling of it.
func TestFoundGroupUnknownToModelsDevNamedByItsID(t *testing.T) {
	spellingHome(t, glmCatalog, "acme-9-1", "acme-9.1")
	g := foundGroup(t, "auto-acme-9-1")
	if g.Ref() != "auto/acme-9-1" || g.Name != "acme-9-1" {
		t.Errorf("the group is %q named %q", g.Ref(), g.Name)
	}
}
