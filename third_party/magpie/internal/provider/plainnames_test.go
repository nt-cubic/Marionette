package provider

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// With PlainNames (#335: 希望 Codex 模型列表里的显示名可以不带 · routing group
// / · 提供商 后缀) the lists agents get name each model by its name alone —
// Codex's catalog and the other agents' files alike — but two a list would
// call the same keep their provider's, so a picker never shows one name
// twice; taking one of them out of the agent's list leaves the other plain.
// Off, as by default, every name has its provider's after it as before, and
// switching it tells the agents.
func TestPlainNames(t *testing.T) {
	prefsHome(t)
	touched := 0
	catalog.Changed = func() { touched++ }
	t.Cleanup(func() { catalog.Changed = nil })
	if err := SetModelName("a/sol", "My Sol"); err != nil {
		t.Fatal(err)
	}
	touched = 0
	codex := func() []string {
		var out []string
		for _, m := range CodexListed() {
			out = append(out, m.ID+"="+m.Name)
		}
		slices.Sort(out)
		return out
	}
	other := func() []string {
		shown, _ := CatalogFor("opencode")
		out := Labels(shown)
		slices.Sort(out)
		return out
	}

	full := []string{"a/sol=My Sol · A", "b/sol=Sol · B", "group/auto-sol=Sol · routing group"}
	if got := codex(); !slices.Equal(got, full) {
		t.Fatalf("by default: %q", got)
	}

	if err := SetPlainNames(true); err != nil {
		t.Fatal(err)
	}
	if touched != 1 || !settings.Load().PlainNames {
		t.Fatalf("agents told %d times, setting %v", touched, settings.Load().PlainNames)
	}
	// the group found for sol and b's sol are both "Sol": they keep theirs
	want := []string{"a/sol=My Sol", "b/sol=Sol · B", "group/auto-sol=Sol · routing group"}
	if got := codex(); !slices.Equal(got, want) {
		t.Fatalf("plain: %q", got)
	}
	if got := other(); !slices.Equal(got, []string{"My Sol", "Sol · B", "Sol · routing group"}) {
		t.Fatalf("plain, another agent: %q", got)
	}

	// b's taken out of Codex's list: the group is the one Sol there, and
	// named so; another agent still has both
	s := settings.Load()
	s.HiddenModels = map[string][]string{"codex": {"b/sol"}}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if got := codex(); !slices.Equal(got, []string{"a/sol=My Sol", "group/auto-sol=Sol"}) {
		t.Fatalf("plain, b hidden: %q", got)
	}
	if got := other(); !slices.Equal(got, []string{"My Sol", "Sol · B", "Sol · routing group"}) {
		t.Fatalf("plain, another agent: %q", got)
	}

	if err := SetPlainNames(false); err != nil {
		t.Fatal(err)
	}
	if got := codex(); !slices.Equal(got, []string{"a/sol=My Sol · A", "group/auto-sol=Sol · routing group"}) || touched != 2 {
		t.Fatalf("off again: %q, told %d", got, touched)
	}
}
