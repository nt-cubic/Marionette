package provider

import (
	"maps"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// A provider's editor sets where Codex and Claude Code compact on its
// models (#876, hisiling): "*" for all of them and model=size for one,
// the model's own first; those it leaves out come off, another
// provider's stay, a model it doesn't serve is refused, and a save that
// changes nothing tells the agents nothing. A group takes the smallest
// its models have.
func TestModelCompactsSetFromTheEditor(t *testing.T) {
	limitsHome(t)
	for _, id := range []string{"deepseek", "other"} {
		if err := Save(Provider{ID: id, Name: id, Chat: "https://relay.example/v1", Key: "k", Models: []string{"deepseek-v4-flash", "deepseek-v4-pro"}}); err != nil {
			t.Fatal(err)
		}
	}
	if err := SetModelCompacts("other", map[string]int{"deepseek-v4-pro": 300000}); err != nil {
		t.Fatal(err)
	}
	told := 0
	catalog.Changed = func() { told++ }
	t.Cleanup(func() { catalog.Changed = nil })

	if err := SetModelCompacts("deepseek", map[string]int{"*": 500000, "deepseek-v4-flash": 272000}); err != nil {
		t.Fatal(err)
	}
	if told != 1 {
		t.Errorf("told the agents %d times; want once", told)
	}
	want := map[string]int{"deepseek/*": 500000, "deepseek/deepseek-v4-flash": 272000, "other/deepseek-v4-pro": 300000}
	if got := settings.Load().ModelCompacts; !maps.Equal(got, want) {
		t.Errorf("compacts %v; want %v", got, want)
	}
	if got := CompactsOf("deepseek"); !maps.Equal(got, map[string]int{"*": 500000, "deepseek-v4-flash": 272000}) {
		t.Errorf("CompactsOf: %v", got)
	}
	for id, n := range map[string]int{
		"deepseek/deepseek-v4-flash":      272000,
		"magpie/deepseek/deepseek-v4-pro": 500000,
		"deepseek/deepseek-v4-pro[1m]":    500000,
		"other/deepseek-v4-flash":         0,
		"other/deepseek-v4-pro":           300000,
		"nobody/deepseek-v4-pro":          0,
	} {
		if got := CompactSet(id); got != n {
			t.Errorf("CompactSet(%s) = %d; want %d", id, got, n)
		}
	}
	group := func(string) (Group, []Member, bool) {
		dp, _ := Find("deepseek")
		ot, _ := Find("other")
		return Group{}, []Member{
			{Path: []string{"deepseek/deepseek-v4-pro"}, Provider: *dp, Model: "deepseek-v4-pro"},
			{Path: []string{"other/deepseek-v4-pro"}, Provider: *ot, Model: "wire-name"},
			{Path: []string{"other/deepseek-v4-flash"}, Provider: *ot, Model: "deepseek-v4-flash"},
		}, true
	}
	if got := compactSet(settings.Load(), GroupPrefix+"mix", group); got != 300000 {
		t.Errorf("a group: %d; want its smallest, 300000", got)
	}

	told = 0
	if err := SetModelCompacts("deepseek", map[string]int{"*": 500000, "deepseek-v4-flash": 272000}); err != nil || told != 0 {
		t.Errorf("the same again: %v, told %d", err, told)
	}
	if err := SetModelCompacts("deepseek", map[string]int{"typo": 5}); err == nil {
		t.Error("a model deepseek doesn't serve was accepted")
	}
	if got := settings.Load().ModelCompacts; !maps.Equal(got, want) {
		t.Errorf("a refused save changed them: %v", got)
	}
	if err := SetModelCompacts("deepseek", map[string]int{}); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().ModelCompacts; !maps.Equal(got, map[string]int{"other/deepseek-v4-pro": 300000}) {
		t.Errorf("emptied: %v", got)
	}
}

// The threshold for every model can be any size, not 272K alone (#876):
// typing one turns Full window off, 272K or nothing is the default again.
func TestSetCompactAt(t *testing.T) {
	limitsHome(t)
	if err := settings.Save(settings.Settings{FullContext: true}); err != nil {
		t.Fatal(err)
	}
	if err := SetCompactAt(500000); err != nil {
		t.Fatal(err)
	}
	if s := settings.Load(); s.FullContext || s.CompactAt != 500000 || s.Compact() != 500000 || s.Working(1000000) != 500000 || s.Working(200000) != 200000 {
		t.Errorf("at 500K: %+v", s)
	}
	if err := SetCompactAt(settings.WorkingWindow); err != nil {
		t.Fatal(err)
	}
	if s := settings.Load(); s.CompactAt != 0 || s.Compact() != settings.WorkingWindow {
		t.Errorf("at 272K: %+v", s)
	}
	if err := SetCompactAt(-1); err == nil {
		t.Error("a negative size was accepted")
	}
	if s := (settings.Settings{CompactAt: 500000, FullContext: true}); s.Compact() != 0 || s.Working(1000000) != 1000000 {
		t.Errorf("full window: %d", s.Compact())
	}
}
