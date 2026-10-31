package gateway

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A group routed Cheapest first sends one model to the member it costs
// least on at the request's hour (liyuanzheng on Discord: DeepSeek's own
// API at night, another provider in DeepSeek's peak hours). DeepSeek's
// API, listed first, bills deepseek-v4-flash at $0.14/$0.28 off-peak and
// double that at peak (catalog.PeakDeepSeek); Ollama at one $0.20/$0.40.
// Off-peak DeepSeek goes first, at peak Ollama, and the trace tells each
// row's price and whether it was the peak one. In order on the same
// prices keeps the list and tells no price.
func TestCheapestFirstGroupFollowsPeakHours(t *testing.T) {
	fresh(t)
	up := httptest.NewServer(http.HandlerFunc(streamed))
	t.Cleanup(up.Close)
	to, _ := url.Parse(up.URL)
	for _, p := range []provider.Provider{
		{ID: "deepseek", Name: "DeepSeek", Key: "sk-ds", Chat: "https://api.deepseek.com/v1", Models: []string{"deepseek-v4-flash"}},
		{ID: "ollama", Name: "Ollama", Key: "sk-ol", Chat: "https://ollama.com/v1", Models: []string{"deepseek-v4-flash"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	f := func(v float64) *float64 { return &v }
	if err := settings.Save(settings.Settings{ModelPrices: map[string]settings.ModelPrice{
		"deepseek/deepseek-v4-flash": {Input: f(0.14), Output: f(0.28), CacheRead: f(0.028), CacheWrite: f(0)},
		"ollama/deepseek-v4-flash":   {Input: f(0.20), Output: f(0.40), CacheRead: f(0.05), CacheWrite: f(0)},
	}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"deepseek/deepseek-v4-flash", "ollama/deepseek-v4-flash"}, Routing: provider.Cheapest,
	}); err != nil {
		t.Fatal(err)
	}
	s := New()
	s.client = &http.Client{Transport: roundTripFunc(func(r *http.Request) (*http.Response, error) {
		r.URL.Scheme, r.URL.Host = to.Scheme, to.Host
		return http.DefaultTransport.RoundTrip(r)
	})}
	ask := func(at time.Time) *Route {
		t.Helper()
		old := priceClock
		priceClock = func() time.Time { return at }
		defer func() { priceClock = old }()
		code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"x"}]}`)
		if code != 200 {
			t.Fatalf("code %d: %s", code, body)
		}
		return s.trace.routes[len(s.trace.routes)-1]
	}
	// a Wednesday, no Chinese holiday: 14:00 UTC off-peak, 02:00 UTC peak
	offPeak := time.Date(2026, 10, 28, 14, 0, 0, 0, time.UTC)
	peak := time.Date(2026, 10, 28, 2, 0, 0, 0, time.UTC)

	night := ask(offPeak)
	if night.Order[0].Provider != "deepseek" {
		t.Fatalf("off-peak Cheapest first went to %q first, want deepseek: %+v", night.Order[0].Provider, night.Order)
	}
	if p := night.Order[0].Price; p == nil || p.Input != 0.14 || p.Output != 0.28 || p.Peak {
		t.Fatalf("the off-peak row doesn't tell DeepSeek's off-peak price: %+v", p)
	}

	day := ask(peak)
	if day.Order[0].Provider != "ollama" {
		t.Fatalf("at DeepSeek's peak Cheapest first went to %q first, want ollama: %+v", day.Order[0].Provider, day.Order)
	}
	var ds *Weighed
	for i := range day.Order {
		if day.Order[i].Provider == "deepseek" {
			ds = &day.Order[i]
		}
	}
	if ds == nil || ds.Price == nil || ds.Price.Input != 0.28 || ds.Price.Output != 0.56 || !ds.Price.Peak {
		t.Fatalf("the peak row doesn't tell DeepSeek's doubled price: %+v", ds)
	}

	// In order, at the peak hour: the list, and no row tells a price
	if err := provider.SaveGroup(provider.Group{
		Name: "G", Members: []string{"deepseek/deepseek-v4-flash", "ollama/deepseek-v4-flash"}, Routing: provider.Ordered,
	}); err != nil {
		t.Fatal(err)
	}
	inOrder := ask(peak)
	if inOrder.Order[0].Provider != "deepseek" {
		t.Fatalf("In order went to %q first: %+v", inOrder.Order[0].Provider, inOrder.Order)
	}
	for _, w := range inOrder.Order {
		if w.Price != nil {
			t.Fatalf("an In order row tells a price: %+v", w)
		}
	}
}

// Cheapest first is a group's: a provider saved with it keeps Smart, and
// a group keeps it.
func TestCheapestFirstIsAGroupsOnly(t *testing.T) {
	fresh(t)
	if err := provider.Save(provider.Provider{ID: "rl", Name: "Relay", Key: "k", Chat: "https://relay.example/v1", Models: []string{"m"}, Routing: provider.Cheapest}); err != nil {
		t.Fatal(err)
	}
	if p, err := provider.Find("rl"); err != nil || p.Routing != "" {
		t.Fatalf("a provider kept routing cheap, want Smart: %v %+v", err, p)
	}
	if err := provider.SaveGroup(provider.Group{Name: "C", Members: []string{"rl/m"}, Routing: provider.Cheapest}); err != nil {
		t.Fatal(err)
	}
	gs := provider.Groups()
	if len(gs) != 1 || gs[0].Routing != provider.Cheapest {
		t.Fatalf("the group didn't keep Cheapest first: %+v", gs)
	}
}
