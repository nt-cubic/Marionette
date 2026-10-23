package gateway

import (
	"net/http"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// liveModels writes the model list a provider's vendor would have answered,
// where catalog.Live reads it: the truth provider.Available goes by, and so
// what FreeModel judges a model free from.
func liveModels(t *testing.T, id, json string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(catalog.LivePath(id)), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(catalog.LivePath(id), []byte(json), 0o644); err != nil {
		t.Fatal(err)
	}
	catalog.Reset()
}

// A group set to Free first spends what costs nothing before what spends
// the subscription: the member whose model its vendor serves at no cost
// answers first, and the paid one only when that can't. magpie learns which
// is which from the vendor's own list (catalog.Model's Free), through
// provider.FreeModel, filled into routes.Seat.Free only for a routing that
// weighs by it (Strategy.ByFree).
func TestFreeFirstGroupSpendsTheFreeMemberFirst(t *testing.T) {
	fresh(t)
	vendor := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"id":"ok","choices":[{"message":{"role":"assistant","content":"hi"}}]}`))
	})
	serveOn(t, "freev", "sk-free", []string{"m"}, vendor)
	serveOn(t, "paidv", "sk-paid", []string{"m"}, vendor)
	// the vendor's own word on one, none on the other: the second is paid
	liveModels(t, "freev", `{"models":[{"id":"m","free":true}]}`)
	liveModels(t, "paidv", `{"models":[{"id":"other"},{"id":"m"}]}`)

	// paid is listed first, so In order would spend it; Free first must not
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"paidv/m", "freev/m"}, Routing: provider.FreeFirst,
	}); err != nil {
		t.Fatal(err)
	}

	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"x"}]}`)
	if code != 200 {
		t.Fatalf("code %d: %s", code, body)
	}
	last := s.trace.routes[len(s.trace.routes)-1]
	if len(last.Order) < 2 {
		t.Fatalf("the trace should weigh both members: %+v", last.Order)
	}
	if last.Order[0].Provider != "freev" {
		t.Fatalf("Free first went to %q first, want freev: %+v", last.Order[0].Provider, last.Order)
	}
	if !last.Order[0].Free {
		t.Fatal("the free member's trace row doesn't say Free")
	}
	if last.Order[len(last.Order)-1].Provider != "paidv" {
		t.Fatalf("the paid member isn't last: %+v", last.Order)
	}
	if last.Order[len(last.Order)-1].Free {
		t.Fatal("the paid member's trace row says Free")
	}
}

// The same group routed In order spends the paid member, which is listed
// first: nothing about the change moves a pool that doesn't ask for it.
func TestInOrderStillGoesByTheListOverTheFreeOne(t *testing.T) {
	fresh(t)
	vendor := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"id":"ok","choices":[{"message":{"role":"assistant","content":"hi"}}]}`))
	})
	serveOn(t, "freev", "sk-free", []string{"m"}, vendor)
	serveOn(t, "paidv", "sk-paid", []string{"m"}, vendor)
	// the vendor's own word on one, none on the other: the second is paid
	liveModels(t, "freev", `{"models":[{"id":"m","free":true}]}`)
	liveModels(t, "paidv", `{"models":[{"id":"other"},{"id":"m"}]}`)
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"paidv/m", "freev/m"}, Routing: provider.Ordered,
	}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"x"}]}`)
	if code != 200 {
		t.Fatalf("code %d: %s", code, body)
	}
	last := s.trace.routes[len(s.trace.routes)-1]
	if last.Order[0].Provider != "paidv" {
		t.Fatalf("In order went to %q first, want paidv: %+v", last.Order[0].Provider, last.Order)
	}
	// In order doesn't weigh by Free: no row says it
	for _, w := range last.Order {
		if w.Free {
			t.Fatalf("a row of an In order trace says Free: %+v", w)
		}
	}
}
