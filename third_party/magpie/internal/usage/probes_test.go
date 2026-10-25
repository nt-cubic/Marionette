package usage

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/probelog"
)

// ledgerSpy is an Observer that only counts the rows Append hands it.
type ledgerSpy struct{ rows []Record }

func (*ledgerSpy) Serve() (stop func()) { return func() {} }
func (*ledgerSpy) Bodies() Bodies       { var b Bodies; return b }
func (*ledgerSpy) Begin(Request) Watch  { return nil }
func (s *ledgerSpy) Ledger(r Record)    { s.rows = append(s.rows, r) }

// A request magpie sends a provider on its own (the editor's Test, Detect
// APIs, a model list) is a row of the request log, marked as magpie's own
// with what it was for, and counts in nothing: no total, source, group,
// summary, Load, LastSeen, shared day or observer (neiko, gys619).
func TestProbeRowsListedNotCounted(t *testing.T) {
	reportedHome(t)
	spy := &ledgerSpy{}
	t.Cleanup(Observe(spy))
	now := time.Now().Add(-time.Hour)
	Append(Record{Time: now, Agent: "claude", Provider: "anthropic", Model: "claude-opus-5-5", Input: 1000, Output: 100, Status: 200})
	probelog.Sink(probelog.Row{Time: now.Add(time.Second), Kind: probelog.KindTest, Provider: "relay", Account: "me", Model: "gpt-9",
		Host: "relay.example", Endpoint: "/v1/chat/completions", Status: 401, Millis: 120, Error: strings.Repeat("页", 400)})
	probelog.Sink(probelog.Row{Time: now.Add(2 * time.Second), Kind: probelog.KindList, Provider: "relay", Host: "relay.example", Endpoint: "GET /v1/models", Status: 200, Millis: 30})

	page := QueryPage(All, Filter{}, 0, 50)
	if page.Total != 3 || len(page.Rows) != 3 {
		t.Fatalf("total %d, %d rows: %+v", page.Total, len(page.Rows), page.Rows)
	}
	var probes []Row
	for _, r := range page.Rows {
		if r.Source == SourceProbe {
			probes = append(probes, r)
		}
	}
	if len(probes) != 2 {
		t.Fatalf("probe rows %+v", page.Rows)
	}
	test := probes[1] // newest first
	if probes[0].Kind != probelog.KindList || test.Kind != probelog.KindTest || test.Agent != "magpie" || test.Provider != "relay" ||
		test.ProviderAccount != "me" || test.Model != "gpt-9" || test.Status != 401 || test.Endpoint != "/v1/chat/completions" {
		t.Fatalf("probe rows %+v", probes)
	}
	if len(test.Error) > 504 || !strings.HasSuffix(test.Error, "…") || !strings.HasPrefix(test.Error, "页") {
		t.Fatalf("error not clipped at a character: %d bytes", len(test.Error))
	}
	if page.Sum.Calls != 1 || page.Sum.Errors != 0 || page.Through.Calls != 1 || page.Direct.Calls != 0 || page.Reported.Calls != 0 {
		t.Fatalf("sum %+v through %+v direct %+v", page.Sum, page.Through, page.Direct)
	}
	for dim, shares := range page.By {
		for _, s := range shares {
			if s.ID == "relay" || s.ID == "magpie" || s.ID == "gpt-9" {
				t.Fatalf("%s counts a probe: %+v", dim, shares)
			}
		}
	}
	if s := Summarize(All); s.Calls != 1 || s.Errors != 0 {
		t.Fatalf("summary %+v", s.Totals)
	}
	if rs := Load(time.Time{}); len(rs) != 1 {
		t.Fatalf("Load %+v", rs)
	}
	if !LastSeen("magpie").IsZero() {
		t.Fatal("a probe is a request from agent magpie")
	}
	n := 0
	for _, calls := range OwnDays(now.Add(-time.Hour)) {
		n += len(calls)
	}
	if n != 1 {
		t.Fatalf("%d shared calls", n)
	}
	if len(spy.rows) != 1 || spy.rows[0].IsProbe() {
		t.Fatalf("observer saw %+v", spy.rows)
	}
	// a filter on the probe's provider lists it and still counts nothing
	if p := QueryPage(All, Filter{Provider: "relay"}, 0, 50); p.Total != 2 || p.Sum.Calls != 0 {
		t.Fatalf("provider filter: total %d sum %+v", p.Total, p.Sum)
	}
}
