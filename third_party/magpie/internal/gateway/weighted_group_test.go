package gateway

import (
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/routes"
)

// A group routed by weight (#1910, DD on Discord): members weighing 3, 1
// and 4 take three, one and four of every eight requests, each tried
// whole — a member's keys in its provider's order, a group in it in its
// own — the next one the fallback. A member resting leaves the round,
// the others sharing it by their weights, and a conversation kept on a
// member counts against that member, so new ones make up the share.
func TestWeightedGroup(t *testing.T) {
	forgetSinks(t)
	s := &Server{}
	url := "http://127.0.0.1:1/v1"
	a := provider.Provider{ID: "wa", Chat: url, Key: "k-a"}
	b := provider.Provider{ID: "wb", Chat: url, Key: "k-b1", Keys: []provider.KeyAccount{{Key: "k-b2", Weight: 50}}, Routing: provider.Ordered}
	c := provider.Provider{ID: "wc", Chat: url, Key: "k-c"}
	d := provider.Provider{ID: "wd", Chat: url, Key: "k-d"}
	sub := provider.Group{ID: "wsub", Routing: provider.Ordered}
	g := provider.Group{ID: "wg", Routing: provider.Weighted, Members: []string{"wa/m", "wb/m:high", "group/wsub"},
		Weights: map[string]int{"wa/m": 3, "group/wsub": 4}}
	ms := []provider.Member{
		{ID: "wa/m", Path: []string{"wa/m"}, Provider: a, Model: "m"},
		{ID: "wb/m:high", Path: []string{"wb/m:high"}, Provider: b, Model: "m", Effort: "high"},
		{ID: "group/wsub", Path: []string{"group/wsub", "wc/m"}, Via: []provider.Group{sub}, Provider: c, Model: "m"},
		{ID: "group/wsub", Path: []string{"group/wsub", "wd/m"}, Via: []provider.Group{sub}, Provider: d, Model: "m"},
	}
	t.Cleanup(func() {
		routes.Forget(provider.GroupPrefix + g.ID)
	})
	unitOf := map[string]string{"wa": "a", "wb": "b", "wc": "sub", "wd": "sub"}
	plan := func() []candidate {
		t.Helper()
		cs, pl := s.planGroup(g, ms, provider.Chat)
		if len(cs) != 5 || len(pl.order) != 5 {
			t.Fatalf("%d planned, %d in the trace: %s", len(cs), len(pl.order), restsOf(cs))
		}
		// each member whole, in its own order
		var ids []string
		for _, c := range cs {
			ids = append(ids, c.p.ID)
		}
		joined := strings.Join(ids, " ")
		if !strings.Contains(joined, "wb wb") || !strings.Contains(joined, "wc wd") {
			t.Fatalf("a member split up: %s", joined)
		}
		if i := slices.Index(ids, "wb"); cs[i].p.Key != "k-b1" || cs[i+1].p.Key != "k-b2" {
			t.Fatalf("wb's keys not in its own order: %s", restsOf(cs))
		}
		if !pl.order[0].Turn || slices.ContainsFunc(pl.order[1:], func(w Weighed) bool { return w.Turn }) {
			t.Fatalf("its turn: %+v", pl.order)
		}
		return cs
	}
	var seq []string
	counts := func(n int, keep func(i int, cs []candidate) candidate) map[string]int {
		got := map[string]int{}
		seq = nil
		for i := range n {
			cs := plan()
			took := cs[0]
			if keep != nil {
				took = keep(i, cs)
			}
			got[unitOf[took.p.ID]]++
			seq = append(seq, unitOf[took.p.ID])
		}
		return got
	}
	if got := counts(8*25, nil); got["a"] != 75 || got["b"] != 25 || got["sub"] != 100 {
		t.Fatalf("3:1:4 over 200: %v", got)
	}
	// smoothly: never the same member three times running
	if strings.Contains(strings.Join(seq, " "), "sub sub sub") {
		t.Fatalf("not smooth: %v", seq[:8])
	}

	// wa resting: wb and the group in it share its requests 1 : 4, and
	// wa is tried last
	var aKey string
	for _, c := range plan() {
		if c.p.ID == "wa" {
			aKey = c.restKey()
		}
	}
	restingUntil.Lock()
	restingUntil.m[aKey] = time.Now().Add(time.Hour)
	restingUntil.Unlock()
	if cs := plan(); cs[len(cs)-1].p.ID != "wa" {
		t.Fatalf("the resting member isn't last: %s", restsOf(cs))
	}
	routes.Forget(provider.GroupPrefix + g.ID)
	if got := counts(5*20, nil); got["a"] != 0 || got["b"] != 20 || got["sub"] != 80 {
		t.Fatalf("with wa resting, 1:4 over 100: %v", got)
	}
	rested(aKey)

	// a conversation kept on wb each round of eight: the round counts it
	// against wb
	routes.Forget(provider.GroupPrefix + g.ID)
	got := counts(8*10, func(i int, cs []candidate) candidate {
		if i%8 != 0 || cs[0].p.ID == "wb" {
			return cs[0]
		}
		at := slices.IndexFunc(cs, func(c candidate) bool { return c.p.ID == "wb" })
		weightedKept(cs[0], cs[at])
		return cs[at]
	})
	if got["a"] != 30 || got["b"] != 10 || got["sub"] != 40 {
		t.Fatalf("with wb kept: %v", got)
	}
}
