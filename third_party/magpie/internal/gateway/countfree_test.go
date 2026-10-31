package gateway

import (
	"net/http"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// A provider the user counts free (provider.CountFree; jiabubu on Discord:
// ModelScope's free daily quota) is spent first by Free first, though its
// vendor's list says nothing of it being free and it is listed second. Its
// price stays as set: the mark orders the routing, it doesn't
// reprice what Usage counts. Cheapest first weighs it at
// nothing too. Turned off, the listed order is back.
func TestFreeFirstSpendsTheProviderTheUserCountsFree(t *testing.T) {
	fresh(t)
	var paid, mine atomic.Int32
	answer := func(n *atomic.Int32) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			n.Add(1)
			w.Header().Set("Content-Type", "application/json")
			w.Write([]byte(`{"id":"ok","choices":[{"message":{"role":"assistant","content":"hi"}}]}`))
		}
	}
	serveOn(t, "paidv", "sk-paid", []string{"m"}, answer(&paid))
	serveOn(t, "mine", "sk-mine", []string{"m"}, answer(&mine))
	// both priced, the marked one dearer: nothing but the user's word says
	// it costs them nothing
	for id, pr := range map[string]catalog.Price{"paidv/m": {Input: 1, Output: 2}, "mine/m": {Input: 3, Output: 15}} {
		if err := provider.SetModelPrice(id, &pr); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"paidv/m", "mine/m"}, Routing: provider.FreeFirst,
	}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetCountFree("mine", true); err != nil {
		t.Fatal(err)
	}

	s := New()
	first := func() Weighed {
		t.Helper()
		code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"x"}]}`)
		if code != 200 {
			t.Fatalf("code %d: %s", code, body)
		}
		last := s.trace.routes[len(s.trace.routes)-1]
		if len(last.Order) != 2 {
			t.Fatalf("the trace should weigh both members: %+v", last.Order)
		}
		return last.Order[0]
	}
	if w := first(); w.Provider != "mine" || !w.Free {
		t.Fatalf("Free first went to %q (free %v) first, want the one counted free", w.Provider, w.Free)
	}
	if mine.Load() != 1 || paid.Load() != 0 {
		t.Fatalf("answered by mine %d, paid %d", mine.Load(), paid.Load())
	}
	// what Usage counts a call at
	if pr, ok := provider.EffectivePrice("mine", "m"); !ok || pr.Input != 3 || pr.Output != 15 {
		t.Fatalf("counting it free repriced it: %+v %v", pr, ok)
	}

	// Cheapest first weighs it at nothing too, though its set price is
	// the dearer
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"paidv/m", "mine/m"}, Routing: provider.Cheapest,
	}); err != nil {
		t.Fatal(err)
	}
	if w := first(); w.Provider != "mine" {
		t.Fatalf("Cheapest first went to %q first, want the one counted free", w.Provider)
	}
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"paidv/m", "mine/m"}, Routing: provider.FreeFirst,
	}); err != nil {
		t.Fatal(err)
	}

	if err := provider.SetCountFree("mine", false); err != nil {
		t.Fatal(err)
	}
	if w := first(); w.Provider != "paidv" || w.Free {
		t.Fatalf("turned off, Free first went to %q (free %v) first, want paidv as listed", w.Provider, w.Free)
	}
}
