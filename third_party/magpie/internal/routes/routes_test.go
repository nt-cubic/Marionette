package routes

import (
	"slices"
	"testing"
	"time"
)

func seats(n int) []Seat {
	out := make([]Seat, n)
	for i := range out {
		out[i] = Seat{Key: string(rune('a' + i))}
	}
	return out
}

// Registering stacks: the last one serves until it is removed, and the one
// under it serves again; removing twice removes once. Smart is served with
// nothing registered, and a routing with none registered is not.
func TestRegisterStacks(t *testing.T) {
	if s, ok := Find(""); !ok || s.Order == nil {
		t.Fatal("Smart isn't served")
	}
	if Served("t-stack") {
		t.Fatal("served with none registered")
	}
	first := Register("t-stack", Strategy{Order: func(string, []Seat) []int { return []int{1, 0} }})
	second := Register("t-stack", Strategy{Order: func(string, []Seat) []int { return []int{0, 1} }, Exact: true})
	if s, _ := Find("t-stack"); !s.Exact {
		t.Fatal("the last registered doesn't serve")
	}
	second()
	second()
	if s, ok := Find("t-stack"); !ok || s.Exact {
		t.Fatalf("after removing the last, the first doesn't serve: %v", ok)
	}
	first()
	if Served("t-stack") {
		t.Fatal("still served after both were removed")
	}
}

// A strategy that panics, or answers what isn't a permutation of its
// seats, leaves them in their order: every route is still tried.
func TestApplyFailsOpen(t *testing.T) {
	for name, order := range map[string]func(string, []Seat) []int{
		"panics":    func(string, []Seat) []int { panic("boom") },
		"short":     func(string, []Seat) []int { return []int{2, 1} },
		"repeats":   func(string, []Seat) []int { return []int{2, 2, 0} },
		"out of it": func(string, []Seat) []int { return []int{3, 1, 0} },
		"negative":  func(string, []Seat) []int { return []int{-1, 1, 0} },
		"nil":       func(string, []Seat) []int { return nil },
	} {
		if got := Apply(Strategy{Order: order}, "p", seats(3)); !slices.Equal(got, []int{0, 1, 2}) {
			t.Errorf("%s: %v, want the listed order", name, got)
		}
	}
	if got := Apply(Strategy{Order: func(string, []Seat) []int { return []int{2, 0, 1} }}, "p", seats(3)); !slices.Equal(got, []int{2, 0, 1}) {
		t.Errorf("a good order was changed: %v", got)
	}
}

// Smart: those with room by when they renew, soonest first, unknown last;
// then those running low, then those all but used up, each by use. One
// learning goes first.
func TestSmart(t *testing.T) {
	now := time.Now()
	s := []Seat{
		{Key: "spent", Used: 99},
		{Key: "later", Used: 10, Soon: []time.Time{now.Add(48 * time.Hour)}},
		{Key: "low", Used: 92},
		{Key: "unknown"},
		{Key: "sooner", Used: 50, Soon: []time.Time{now.Add(2 * time.Hour)}},
		{Key: "learns", Learns: true},
	}
	got := keys(s, Apply(Smart, "p", s))
	if want := []string{"learns", "sooner", "later", "unknown", "low", "spent"}; !slices.Equal(got, want) {
		t.Fatalf("Smart: %v, want %v", got, want)
	}
}

func keys(s []Seat, order []int) []string {
	out := make([]string, len(order))
	for i, j := range order {
		out[i] = s[j].Key
	}
	return out
}

// In turn starts one further along each time, per pool.
func TestRotate(t *testing.T) {
	Forget("r1", "r2")
	s := seats(3)
	var got [][]string
	for range 4 {
		got = append(got, keys(s, Apply(Rotate, "r1", s)))
	}
	want := [][]string{{"a", "b", "c"}, {"b", "c", "a"}, {"c", "a", "b"}, {"a", "b", "c"}}
	for i := range want {
		if !slices.Equal(got[i], want[i]) {
			t.Fatalf("turn %d: %v, want %v", i, got[i], want[i])
		}
	}
	if g := keys(s, Apply(Rotate, "r2", s)); g[0] != "a" {
		t.Fatalf("another pool shares the turn: %v", g)
	}
	Forget("r1")
	if g := keys(s, Apply(Rotate, "r1", s)); g[0] != "a" {
		t.Fatalf("Forget kept the turn: %v", g)
	}
}

// Least used first by share used, then by tokens; in order is the listed
// order, and exact.
func TestLeastUsedAndOrdered(t *testing.T) {
	s := []Seat{{Key: "a", Used: 50}, {Key: "b", Used: 10, Tokens: 9}, {Key: "c", Used: 10, Tokens: 1}}
	if got := keys(s, Apply(LeastUsed, "p", s)); !slices.Equal(got, []string{"c", "b", "a"}) {
		t.Fatalf("least used: %v", got)
	}
	if got := keys(s, Apply(Ordered, "p", s)); !slices.Equal(got, []string{"a", "b", "c"}) || !Ordered.Exact {
		t.Fatalf("in order: %v", got)
	}
}

// Weekly pace: the most of a week left per hour first; one running low
// after, whatever its pace.
func TestPace(t *testing.T) {
	s := []Seat{{Key: "slow", Pace: 1}, {Key: "low", Used: 95, Pace: 50}, {Key: "fast", Pace: 10}}
	if got := keys(s, Apply(Pace, "p", s)); !slices.Equal(got, []string{"fast", "slow", "low"}) {
		t.Fatalf("pace: %v", got)
	}
}

// By weight: over six requests a seat weighing 2 goes first twice as often
// as each weighing 1, and the others follow the one picked in their order.
func TestWeighted(t *testing.T) {
	Forget("w")
	s := []Seat{{Key: "a", Weight: 2}, {Key: "b", Weight: 1}, {Key: "c", Weight: 1}}
	firsts := map[string]int{}
	for range 8 {
		order := keys(s, Apply(Weighted, "w", s))
		firsts[order[0]]++
		rest := slices.DeleteFunc(slices.Clone([]string{"a", "b", "c"}), func(k string) bool { return k == order[0] })
		if !slices.Equal(order[1:], rest) {
			t.Fatalf("the others aren't in their order: %v", order)
		}
	}
	if firsts["a"] != 4 || firsts["b"] != 2 || firsts["c"] != 2 {
		t.Fatalf("by weight: %v, want a 4, b 2, c 2", firsts)
	}
}

// Of those all but used up, one that goes on on its credits (a Codex
// account holding them) goes before those with nothing left, though it
// has used more, in Smart, Weekly pace and Least used alike; still after
// every one with room or running low, its allowance spent before its
// credits are.
func TestCreditsFirstOfTheSpent(t *testing.T) {
	s := []Seat{
		{Key: "spent", Used: 98},
		{Key: "credits", Used: 100, Credits: true},
		{Key: "low", Used: 92, Credits: true},
		{Key: "fine", Used: 20},
	}
	for name, st := range map[string]Strategy{"smart": Smart, "pace": Pace, "least used": LeastUsed} {
		if got := keys(s, Apply(st, "p", s)); !slices.Equal(got, []string{"fine", "low", "credits", "spent"}) {
			t.Errorf("%s: %v", name, got)
		}
	}
}
