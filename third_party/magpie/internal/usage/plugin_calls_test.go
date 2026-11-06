package usage

import (
	"os"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/plugin"
)

// A cache keepalive ping the Devin plugin sent on its own (plugins#592) is
// a row of magpie's on the request log, labelled by its kind, and counted:
// its cache read is in the totals and priced as the plugin's turns are
// (here at the model's maker's list price), as the user who turned the
// keepalive on is to see what it costs (zelong77).
func TestPluginKeepaliveIsCounted(t *testing.T) {
	pageHome(t)
	os.WriteFile(catalog.CachePath(), []byte(`{"anthropic":{"id":"anthropic","models":{"claude-sonnet-4-5":{"id":"claude-sonnet-4-5","cost":{"input":3,"output":15,"cache_read":0.3,"cache_write":3.75}}}}}`), 0600)
	now := holdClock(t, time.Date(2026, 11, 1, 12, 0, 0, 0, time.Local))
	r := pluginCall("devin", "devin#ab", plugin.OwnCall{Kind: KindKeepalive, Model: "claude-sonnet-4-5", Input: 3, Output: 1, CacheRead: 10984, Status: 200, Millis: 812})
	r.Time = Today.Since(now).Add(time.Second)
	Append(r)
	Append(Record{Time: r.Time.Add(time.Second), Agent: "claude-code", Provider: "devin", Model: "claude-sonnet-4-5", Input: 5, Output: 9, CacheWrite: 10984, Status: 200})
	p := QueryPage(Today, Filter{Purpose: "kind:" + KindKeepalive}, 0, 10)
	if p.Total != 1 || len(p.Rows) != 1 || p.Sum.Calls != 1 || p.Sum.CacheRead != 10984 || p.Sum.Input != 3 || p.Sum.Output != 1 || p.Sum.Cost <= 0 {
		t.Fatalf("keepalive page = %+v", p)
	}
	if row := p.Rows[0]; row.Kind != KindKeepalive || row.Agent != "magpie" || row.Provider != "devin" || row.Model != "claude-sonnet-4-5" || row.Failed() || row.IsProbe() {
		t.Fatalf("keepalive row = %+v", row)
	}
	if all := QueryPage(Today, Filter{}, 0, 10); all.Sum.Calls != 2 || all.Sum.CacheRead != 10984 {
		t.Fatalf("all calls = %+v, want the ping counted beside the turn", all.Sum)
	}
	if k := PurposeKinds()[KindKeepalive]; k.Name != "Cache keep-alive" || k.Purpose != "kind:"+KindKeepalive {
		t.Fatalf("keepalive purpose = %+v", k)
	}
}
