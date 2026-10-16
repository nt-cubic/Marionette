package tui

import (
	"fmt"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The group page keeps the row picked in view, and its header with it: a
// group with more members and rules than the terminal has rows can't
// scroll them off the top and freeze the page, as the library page did
// (#2220) and the agents page before it (#2428). The page takes no more
// lines than a short group's, which fills the terminal exactly; what a
// terminal shows of a view taller than itself is its last lines.
func TestGroupViewKeepsTheRowPicked(t *testing.T) {
	home(t)
	var members []string
	for i := 0; i < 30; i++ {
		members = append(members, fmt.Sprintf("p/m-%02d", i))
	}
	var rules []provider.Rule
	for i := 0; i < 24; i++ {
		rules = append(rules, provider.Rule{Use: "p/m-00", Agents: []string{fmt.Sprintf("agent-%02d", i)}})
	}
	// the reporter's group (#2452): a whole provider's thirty models, no rules
	pageKeepsThePick(t, provider.Group{ID: "many", Name: "Many", Members: members})
	// one model and many rules: a pick among the rules
	pageKeepsThePick(t, provider.Group{ID: "ruled", Name: "Ruled", Members: members[:1], Rules: rules})
	// both, with patterns and an intent rule's classifier line around them
	withIntent := append([]provider.Rule{{Use: "p/m-01", Intent: "a quick question"}}, rules...)
	pageKeepsThePick(t, provider.Group{ID: "full", Name: "Full", Members: members, Rules: withIntent, Classifier: "a/m",
		Match: []string{"nothing-a-*", "nothing-b-*", "nothing-c-*"}})
}

func pageKeepsThePick(t *testing.T, g provider.Group) {
	t.Helper()
	if err := provider.SaveGroup(provider.Group{ID: "short", Name: "Short", Members: g.Members[:1]}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(g); err != nil {
		t.Fatal(err)
	}
	for _, h := range []int{24, 40} {
		short := openGroup(t, "short", h)
		room := strings.Count(short.View(), "\n")
		m := openGroup(t, g.ID, h)
		for pick := 0; pick < len(g.Members)+len(g.Rules); pick++ {
			m.gsel = pick
			v := m.View()
			if n := strings.Count(v, "\n"); n > room {
				t.Fatalf("%s, %d rows: pick %d draws %d lines, a short group's page %d:\n%s", g.ID, h, pick, n, room, v)
			}
			if !strings.Contains(v, "◉ magpie") {
				t.Fatalf("%s, %d rows: pick %d scrolled the header off:\n%s", g.ID, h, pick, v)
			}
			want := ""
			if pick < len(g.Members) {
				want = g.Members[pick]
			} else {
				want = g.Rules[pick-len(g.Members)].Use
			}
			line := ""
			for _, l := range strings.Split(v, "\n") {
				if strings.Contains(l, "▸ ") {
					line = l
				}
			}
			if !strings.Contains(line, fmt.Sprintf("%d  ", pickNumber(g, pick))) || !strings.Contains(line, want) {
				t.Fatalf("%s, %d rows: pick %d (%s) isn't the row marked (%q):\n%s", g.ID, h, pick, want, line, v)
			}
		}
	}
	// a terminal tall enough draws the whole group, from its first model
	m := openGroup(t, g.ID, len(g.Members)+len(g.Rules)+len(g.Match)+20)
	m.gsel = len(g.Members) + len(g.Rules) - 1
	v := m.View()
	if !strings.Contains(v, "1  "+g.Members[0]) || !strings.Contains(v, g.Members[len(g.Members)-1]) {
		t.Fatalf("%s: a tall terminal left part of the group out:\n%s", g.ID, v)
	}
}

// pickNumber is the number the page shows beside a pick: its place among
// the members, or among the rules.
func pickNumber(g provider.Group, pick int) int {
	if pick < len(g.Members) {
		return pick + 1
	}
	return pick - len(g.Members) + 1
}

// openGroup is the group page for id, on a terminal h rows tall.
func openGroup(t *testing.T, id string, h int) model {
	t.Helper()
	m := press(t, model{w: 120, h: h}, "3")
	for i, g := range m.groups {
		if g.ID == id {
			m.grow = i
		}
	}
	m = press(t, m, "enter")
	if m.mode != modeGroup {
		t.Fatalf("mode %d, want the group page", m.mode)
	}
	if g, _ := m.group(); g.ID != id {
		t.Fatalf("opened %s, want %s", g.ID, id)
	}
	return m
}
