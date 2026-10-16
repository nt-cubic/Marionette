package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A provider set to route its own way keeps it inside a routing group,
// whatever the group's routing, as a group in the group does (kchen0x on
// X: Antigravity In order inside a Smart group still went to its ninth
// account, the one Smart put first). The group weighs it as one, by the
// first it would try, and tries it whole in its own order. One left on
// Smart is weighed key by key with the rest, as before.
func TestProviderKeepsItsOwnRoutingInAGroup(t *testing.T) {
	now := time.Now()
	week := func(in time.Duration) provider.Allowance {
		return provider.Allowance{{Used: 10, Resets: now.Add(in), Span: 7 * 24 * time.Hour}}
	}
	// Smart alone would go k3, k2, k1: k3 renews soonest
	windows := map[string]provider.Allowance{
		"sk-test-k1": week(4 * 24 * time.Hour), "sk-test-k2": week(3 * 24 * time.Hour), "sk-test-k3": week(time.Hour),
		"sk-test-kb": week(6 * 24 * time.Hour),
	}
	order := func(s *Server) string {
		r := s.trace.routes[len(s.trace.routes)-1]
		var out []string
		for _, w := range r.Order {
			if w.Provider == "biz" { // its one key, told as the provider
				out = append(out, "kb")
				continue
			}
			out = append(out, w.Who[len(w.Who)-2:]) // masked: its last two
		}
		return strings.Join(out, " ")
	}
	keysOf := func(o string) string { // ag's keys, in the order tried
		var out []string
		for _, k := range strings.Fields(o) {
			if k != "kb" {
				out = append(out, k)
			}
		}
		return strings.Join(out, " ")
	}
	for _, outer := range []string{"", provider.Rotate, provider.LeastUsed, provider.Pace, provider.Manual} {
		t.Run("group "+outer, func(t *testing.T) {
			fresh(t)
			keyWindows(t, windows)
			ag, biz := &keyed{}, &keyed{}
			serveOn(t, "ag", "sk-test-k1", []string{"m"}, ag, "sk-test-k2", "sk-test-k3")
			serveOn(t, "biz", "sk-test-kb", []string{"m"}, biz)
			if err := provider.SetRouting("ag", provider.Ordered); err != nil {
				t.Fatal(err)
			}
			if err := provider.SaveGroup(provider.Group{Name: "Flash", Members: []string{"ag/m", "biz/m"}, Routing: outer, Pick: "ag/m"}); err != nil {
				t.Fatal(err)
			}
			s := New()
			for i := range 3 {
				code, body := postAs(t, s, "s"+string(rune('a'+i)), `{"model":"group/flash","messages":[{"role":"user","content":"hi"}]}`)
				if code != 200 {
					t.Fatalf("%d %s", code, body)
				}
				o := order(s)
				if keysOf(o) != "k1 k2 k3" {
					t.Fatalf("request %d: %s, ag's keys not in its own order", i+1, o)
				}
				if outer == provider.Manual {
					if o != "k1 k2 k3" { // the member picked alone
						t.Fatalf("manual: %s", o)
					}
					continue
				}
				if o != "k1 k2 k3 kb" && o != "kb k1 k2 k3" {
					t.Fatalf("request %d: %s, ag not tried whole", i+1, o)
				}
				if outer == "" && o != "k1 k2 k3 kb" {
					t.Fatalf("smart: %s, ag's first renews before kb", o)
				}
			}
			if outer == "" && (len(ag.tried) != 3 || ag.tried[0] != "sk-test-k1") {
				t.Fatalf("smart sent to %v", ag.tried)
			}
		})
	}

	// ag on Smart is weighed key by key with biz, as one pool
	fresh(t)
	keyWindows(t, windows)
	serveOn(t, "ag", "sk-test-k1", []string{"m"}, &keyed{}, "sk-test-k2", "sk-test-k3")
	serveOn(t, "biz", "sk-test-kb", []string{"m"}, &keyed{})
	if err := provider.SaveGroup(provider.Group{Name: "Flash", Members: []string{"biz/m", "ag/m"}}); err != nil {
		t.Fatal(err)
	}
	s := New()
	if code, body := postAs(t, s, "pool", `{"model":"group/flash","messages":[{"role":"user","content":"hi"}]}`); code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	if o := order(s); o != "k3 k2 k1 kb" {
		t.Fatalf("pooled: %s", o)
	}
}
