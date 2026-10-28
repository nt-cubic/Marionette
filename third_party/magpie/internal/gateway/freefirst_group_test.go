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

// A group in a Free first group that sets no routing of its own is routed
// as the group it is in, as a provider left on the default is (#3847,
// mycf_711): its free seat goes first and says Free, and the trace's
// group in the group says Free first, so the Routing page reads it as
// such. One that sets its own routing keeps it.
func TestFreeFirstReachesAGroupInItLeftOnTheDefault(t *testing.T) {
	fresh(t)
	vendor := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"id":"ok","choices":[{"message":{"role":"assistant","content":"hi"}}]}`))
	})
	serveOn(t, "freev", "sk-free", []string{"m"}, vendor)
	serveOn(t, "paidv", "sk-paid", []string{"m"}, vendor)
	liveModels(t, "freev", `{"models":[{"id":"m","free":true}]}`)
	liveModels(t, "paidv", `{"models":[{"id":"other"},{"id":"m"}]}`)
	if err := provider.SaveGroup(provider.Group{Name: "Inner", Members: []string{"paidv/m", "freev/m"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "Outer", Members: []string{"group/inner"}, Routing: provider.FreeFirst}); err != nil {
		t.Fatal(err)
	}
	s := New()
	last := func() *Route {
		code, body := postAs(t, s, "", `{"model":"group/outer","messages":[{"role":"user","content":"x"}]}`)
		if code != 200 {
			t.Fatalf("code %d: %s", code, body)
		}
		return s.trace.routes[len(s.trace.routes)-1]
	}
	r := last()
	if len(r.Order) != 2 || r.Order[0].Provider != "freev" || !r.Order[0].Free || r.Order[0].Routing != provider.FreeFirst {
		t.Fatalf("the free seat inside inner: %+v", r.Order)
	}
	if r.Order[1].Free || r.Order[1].Routing != provider.FreeFirst {
		t.Fatalf("the paid seat inside inner: %+v", r.Order[1])
	}
	if r.Group == nil || len(r.Group.Subs) != 1 || r.Group.Subs[0].Routing != provider.FreeFirst {
		t.Fatalf("the trace's group in the group: %+v", r.Group)
	}

	// inner set In order keeps it: paid first, nobody weighed by Free
	inner, _, _ := provider.FindGroup("group/inner")
	inner.Routing = provider.Ordered
	if err := provider.SaveGroup(inner); err != nil {
		t.Fatal(err)
	}
	r = last()
	if r.Order[0].Provider != "paidv" || r.Group.Subs[0].Routing != provider.Ordered {
		t.Fatalf("inner In order: %+v %+v", r.Order, r.Group.Subs)
	}

	// an In order outer tries inner whole, by inner's own: left on the
	// default, Smart, as a provider's keys in an In order group are
	inner.Routing = ""
	if err := provider.SaveGroup(inner); err != nil {
		t.Fatal(err)
	}
	outer, _, _ := provider.FindGroup("group/outer")
	outer.Routing = provider.Ordered
	if err := provider.SaveGroup(outer); err != nil {
		t.Fatal(err)
	}
	r = last()
	if r.Group.Subs[0].Routing != "" {
		t.Fatalf("inner in an In order group: %+v", r.Group.Subs)
	}
	for _, w := range r.Order {
		if w.Free {
			t.Fatalf("a row says Free under In order: %+v", w)
		}
	}
}
