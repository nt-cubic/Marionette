package provider

import (
	"slices"
	"testing"
)

// With names the user set plain (PlainOwnNames, #92), a routing group they
// made is one of them (#868: 模型名称太长…路由组的名称也很长, an agent's
// narrow menu cut "· routing group" to "· rou…"): its name stands alone in
// the agents' lists. A group magpie found keeps "· routing group", the
// vendors' names keep their providers', and a name the list would show
// twice keeps its suffix, so a picker never shows one name twice.
func TestPlainOwnNamesGroups(t *testing.T) {
	prefsHome(t)
	if err := SetModelName("a/sol", "My Sol"); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{ID: "fast", Name: "Fast", Members: []string{"a/sol", "b/sol"}}); err != nil {
		t.Fatal(err)
	}
	labels := func() []string {
		shown, _ := CatalogFor("opencode")
		out := Labels(shown)
		slices.Sort(out)
		return out
	}
	if got, want := labels(), []string{"Fast · routing group", "My Sol · A", "Sol · B", "Sol · routing group"}; !slices.Equal(got, want) {
		t.Fatalf("by default: %q, want %q", got, want)
	}
	if err := SetSuffixMode(SuffixOwn); err != nil {
		t.Fatal(err)
	}
	if got, want := labels(), []string{"Fast", "My Sol", "Sol · B", "Sol · routing group"}; !slices.Equal(got, want) {
		t.Fatalf("own names plain: %q, want %q", got, want)
	}
	// a group named as a model the list has keeps its suffix
	if err := SaveGroup(Group{ID: "fast", Name: "My Sol", Members: []string{"a/sol", "b/sol"}}); err != nil {
		t.Fatal(err)
	}
	if got, want := labels(), []string{"My Sol · A", "My Sol · routing group", "Sol · B", "Sol · routing group"}; !slices.Equal(got, want) {
		t.Fatalf("a group named as a model: %q, want %q", got, want)
	}
}
