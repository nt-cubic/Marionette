package backup

import (
	"testing"

	"github.com/yetone/magpie/internal/library"
)

// A backup file carries the library's subagents, and restoring it on
// another computer puts them in its library; an older file, made before
// subagents were carried, leaves the ones there.
func TestBackupFileCarriesSubagents(t *testing.T) {
	home(t)
	if _, err := library.SaveSubagent("", library.Subagent{Name: "reviewer", Description: "Reviews a diff",
		Prompt: "Report bugs only.", MCP: []string{"gh"}, Agents: []string{"claude"}}); err != nil {
		t.Fatal(err)
	}
	b, err := Collect(false, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, _ := Seal(b, "pw")
	old := b
	lib := *b.Library
	lib.Subagents = nil // as a magpie before them sealed it
	old.Library = &lib
	older, _ := Seal(old, "pw")

	home(t)
	if _, err := library.SaveSubagent("", library.Subagent{Name: "mine", Description: "Mine", Prompt: "Mine.", Agents: []string{}}); err != nil {
		t.Fatal(err)
	}
	got, _ := Open(older, "pw")
	if _, err := Restore(got, All); err != nil {
		t.Fatal(err)
	}
	have := func() []string {
		l, _ := library.Collect()
		var names []string
		for _, s := range *l.Subagents {
			names = append(names, s.Name)
		}
		return names
	}
	if n := have(); len(n) != 1 || n[0] != "mine" {
		t.Fatalf("an older file changed the subagents: %v", n)
	}
	got, _ = Open(data, "pw")
	if r, err := Restore(got, All); err != nil || !r.Library {
		t.Fatalf("restore: %+v %v", r, err)
	}
	l, _ := library.Collect()
	if s := *l.Subagents; len(s) != 1 || s[0].Name != "reviewer" || s[0].Prompt != "Report bugs only." || len(s[0].MCP) != 1 || s[0].Agents[0] != "claude" {
		t.Fatalf("subagents restored: %+v", s)
	}
}
