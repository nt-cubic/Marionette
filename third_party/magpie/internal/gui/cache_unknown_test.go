package gui

import (
	"slices"
	"testing"

	"github.com/yetone/magpie/internal/gateway"
)

// A Kiro request's prompt is counted as Kiro counts it, prompt tokens but
// no cache, so the Context page scores its agent neither up nor down for
// the cache and doesn't call it cache-misses, and each point says its cache
// is unknown (#3464): the plugin said so (CacheUnknown), or it was kept
// before the plugin did and Kiro never counts it.
func TestContextKiroCacheNotAMiss(t *testing.T) {
	for _, c := range []struct {
		name string
		u    gateway.RouteUsage
	}{
		{"said so", gateway.RouteUsage{Provider: "kiro", Input: 9000, CacheUnknown: true}},
		{"kept before", gateway.RouteUsage{Provider: "kiro", Input: 9000}},
	} {
		t.Run(c.name, func(t *testing.T) {
			var routes []gateway.Route
			for i := range 6 {
				r := ctxRoute(int64(i+1), "claude", "s1", 8000, 1000*i, 1000000, 0)
				r.Usage = []gateway.RouteUsage{c.u}
				routes = append(routes, r)
			}
			out := contextOf(routes, 1)
			a := out.Agents[0]
			if a.Scores[0].Key != "cache" || a.Scores[0].Points != 18 || a.Cache != 0 {
				t.Fatalf("cache %v, score %+v", a.Cache, a.Scores[0])
			}
			if slices.Contains(tagKeys(a), "cache-misses") {
				t.Fatalf("tags %v", tagKeys(a))
			}
			for _, p := range out.Sessions[0].Points {
				if !p.CacheUnknown {
					t.Fatalf("point %+v", p)
				}
			}
		})
	}
}

// The Routing page is sent a Kiro try kept before its plugin said Kiro
// counts no cache as one that doesn't, so it shows no hit rate rather than
// 0% (zeewell on X, #3464). A vendor that counts its cache keeps its 0%,
// and the route read is left as it was.
func TestRoutingKiroCacheKeptBefore(t *testing.T) {
	kept := []gateway.RouteUsage{{Provider: "kiro", Input: 148000, Output: 19}, {Provider: "anthropic", Input: 1200, Output: 1}}
	got := cacheCounted(kept)
	if !got[0].CacheUnknown || got[1].CacheUnknown {
		t.Fatalf("%+v", got)
	}
	if kept[0].CacheUnknown {
		t.Fatal("the route read was changed")
	}
	routes := pricedRoutes([]gateway.Route{{ID: 1, Agent: "claude", Provider: "kiro", Model: "claude-sonnet-4.5", Done: true, Status: 200, Usage: kept[:1]}})
	if len(routes) != 1 || !routes[0].Usage[0].CacheUnknown {
		t.Fatalf("%+v", routes)
	}
}
