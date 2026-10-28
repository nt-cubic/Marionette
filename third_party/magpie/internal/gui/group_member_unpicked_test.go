package gui

import (
	"os"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A group's member the user unchecked in its provider's model list is
// not ready on the Routing page (#3911), as the gateway skips it: a
// manual group's card struck nothing while the editor struck it.
func TestGroupMemberUncheckedInItsProvider(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "a", Name: "A", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x", "y"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "b", Name: "B", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "mine", Name: "Mine", Members: []string{"a/x", "b/x"}, Routing: provider.Manual, Pick: "a/x"}); err != nil {
		t.Fatal(err)
	}
	ready := func() map[string]bool {
		out := map[string]bool{}
		for _, g := range groupsState().Groups {
			if g.ID == "mine" {
				out["group"] = g.Ready
				for _, m := range g.Info {
					out[m.ID] = m.Ready
				}
			}
		}
		return out
	}
	if r := ready(); !r["a/x"] || !r["b/x"] || !r["group"] {
		t.Fatalf("before: %v", r)
	}
	if err := provider.SetModels("a", []string{"y"}); err != nil {
		t.Fatal(err)
	}
	if r := ready(); r["a/x"] || !r["b/x"] || r["group"] {
		t.Fatalf("a/x unchecked: %v", r)
	}
	if _, _, ok := provider.Resolve("a/x"); !ok {
		t.Fatal("a/x named directly no longer resolves")
	}
}
