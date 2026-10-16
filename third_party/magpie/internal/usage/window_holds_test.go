package usage

import (
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// holdsHome is a home whose models.dev copy prices a Claude and an OpenAI
// model as their vendors list them: Anthropic's with its cache writes (5
// minutes 1.25× input, an hour 2×), OpenAI's with cached input and no
// cache write price at all.
func holdsHome(t *testing.T) {
	t.Helper()
	sessionHome(t)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	os.MkdirAll(filepath.Dir(catalog.CachePath()), 0700)
	os.WriteFile(catalog.CachePath(), []byte(`{
"anthropic":{"id":"anthropic","models":{"claude-opus-4-7":{"id":"claude-opus-4-7","cost":{"input":5,"output":25,"cache_read":0.5,"cache_write":6.25}},
  "claude-sonnet-4-6":{"id":"claude-sonnet-4-6","cost":{"input":3,"output":15,"cache_read":0.3,"cache_write":3.75}}}},
"openai":{"id":"openai","models":{"gpt-5.5":{"id":"gpt-5.5","cost":{"input":1.25,"output":10,"cache_read":0.125}}}}}`), 0600)
}

func near(a, b float64) bool { return math.Abs(a-b) <= 1e-9*max(1, math.Abs(b)) }

// Chiao on Discord: each window's whole, reckoned from the calls magpie
// routed through the account in it over the share used, at API prices —
// for Claude's 5 hours, its week and its Opus-only week, Codex's 5 hours,
// and a moved plugin's window told in seconds to its reset; and nothing
// where that wouldn't be honest.
func TestWindowHoldsReckonsWhatMagpieRouted(t *testing.T) {
	holdsHome(t)
	now := holdClock(t, time.Date(2026, 10, 8, 12, 0, 0, 0, time.UTC))
	readAt := now.Add(-2 * time.Minute)
	at := func(d time.Duration) *time.Time { x := now.Add(d); return &x }
	claude := Record{Agent: "claude-code", Provider: "claude", ProviderAccount: "Dee@example.com", Status: 200}
	call := func(base Record, ago time.Duration, model string, in, out, read, write, write1h int) Record {
		base.Time, base.Model, base.Input, base.Output, base.CacheRead, base.CacheWrite, base.CacheWrite1h = now.Add(-ago), model, in, out, read, write, write1h
		return base
	}
	codex := Record{Agent: "codex", Provider: "codex", Host: "chatgpt.com as sam@example.com", Status: 200} // an older record: its account in Host
	kiro := Record{Agent: "claude-code", Provider: "kiro", ProviderAccount: "kay@example.com", Status: 200}
	recs := []Record{
		call(claude, 1*time.Hour, "claude-opus-4-7", 1000, 2000, 50000, 4000, 1000),
		call(claude, 2*time.Hour, "claude-sonnet-4-6", 3000, 1000, 20000, 0, 0),
		call(claude, 6*time.Hour, "claude-opus-4-7", 10000, 5000, 0, 0, 0),             // before the 5 hours, in the week
		call(claude, 8*24*time.Hour, "claude-opus-4-7", 99000, 99000, 0, 0, 0),         // before the week: the log's first call
		call(claude, time.Minute, "claude-opus-4-7", 77000, 77000, 0, 0, 0),            // after the share was read
		{Time: now.Add(-time.Hour), Agent: "claude-code", Status: 429, Rejected: true}, // turned away here
		call(Record{Agent: "claude-code", Provider: "claude", ProviderAccount: "bob@example.com", Status: 200}, time.Hour, "claude-opus-4-7", 5000, 5000, 0, 0, 0),
		call(Record{Agent: "claude-code", Provider: "anthropic", ProviderKeyID: "k1", Status: 200}, time.Hour, "claude-opus-4-7", 5000, 5000, 0, 0, 0),
		call(codex, 30*time.Minute, "gpt-5.5", 4000, 1000, 60000, 2000, 0),
		call(kiro, 3*24*time.Hour, "claude-sonnet-4-6", 6000, 2000, 0, 0, 0),
		call(kiro, 24*time.Hour, "kiro-auto", 1000, 1000, 0, 0, 0), // no API price
	}
	for _, r := range recs {
		Append(r)
	}
	qs := []provider.SubscriptionQuota{
		{Provider: "claude", User: "dee@example.com", ReadAt: &readAt, Windows: []provider.QuotaWindow{
			{Name: "5 hours", Used: 20, ResetsAt: at(2 * time.Hour), Span: 5 * time.Hour},
			{Name: "7 days", Used: 40, ResetsAt: at(3 * 24 * time.Hour), Span: 7 * 24 * time.Hour},
			{Name: "7 days · Opus", Used: 50, ResetsAt: at(3 * 24 * time.Hour), Span: 7 * 24 * time.Hour, Model: "opus"},
			{Name: "Extra usage", Used: 30, ResetsAt: at(20 * 24 * time.Hour), Span: 30 * 24 * time.Hour, Aside: true},
		}},
		{Provider: "codex", User: "sam@example.com", Windows: []provider.QuotaWindow{
			{Name: "5 hours", Used: 10, ResetSecs: 3 * 3600, Span: 5 * time.Hour},
			{Name: "7 days", Used: 4, ResetsAt: at(4 * 24 * time.Hour), Span: 7 * 24 * time.Hour}, // too little used to scale
		}},
		{Provider: "kiro", Name: "Kiro", User: "kay@example.com", Windows: []provider.QuotaWindow{
			{Name: "Weekly", Used: 25, ResetSecs: 2 * 24 * 3600}, // its span from its name
			{Name: "Credits", Used: 30},                          // no reset: no start
		}},
		{Provider: "claude", User: "bob@example.com", Error: "sign in again", Windows: []provider.QuotaWindow{{Name: "5 hours", Used: 50, ResetsAt: at(time.Hour), Span: 5 * time.Hour}}},
	}
	got := WithWindowHolds(qs, now)

	opus := func(in, out, read, write, write1h float64) float64 {
		return (in*5 + out*25 + read*0.5 + (write-write1h)*6.25 + write1h*10) / 1e6
	}
	sonnet := func(in, out, read float64) float64 { return (in*3 + out*15 + read*0.3) / 1e6 }
	want := []struct {
		q, w           int
		tokens, routed int64
		cost           float64 // 0: not priced
		calls          int
	}{
		{0, 0, (3000 + 4000) * 5, 7000, (opus(1000, 2000, 50000, 4000, 1000) + sonnet(3000, 1000, 20000)) * 5, 2},
		{0, 1, (3000 + 4000 + 15000) * 100 / 40, 22000, (opus(1000, 2000, 50000, 4000, 1000) + sonnet(3000, 1000, 20000) + opus(10000, 5000, 0, 0, 0)) * 100 / 40, 3},
		{0, 2, (3000 + 15000) * 2, 18000, (opus(1000, 2000, 50000, 4000, 1000) + opus(10000, 5000, 0, 0, 0)) * 2, 2},
		// OpenAI lists no cache write: what was written is priced at nothing, not at input
		{1, 0, 5000 * 10, 5000, (4000*1.25 + 1000*10 + 60000*0.125) / 1e6 * 10, 1},
		{2, 0, (8000 + 2000) * 4, 10000, 0, 2},
	}
	for _, x := range want {
		w := got[x.q].Windows[x.w]
		h := w.Holds
		if h == nil {
			t.Fatalf("%s %s: no whole reckoned", got[x.q].Provider, w.Name)
		}
		if h.Tokens != x.tokens || h.Routed.Tokens != x.routed || h.Routed.Calls != x.calls || h.Used != w.Used {
			t.Errorf("%s %s: %d tokens whole from %d in %d calls at %v%%, want %d from %d in %d", got[x.q].Provider, w.Name, h.Tokens, h.Routed.Tokens, h.Routed.Calls, h.Used, x.tokens, x.routed, x.calls)
		}
		if h.Priced != (x.cost > 0) || !near(h.Cost, x.cost) {
			t.Errorf("%s %s: cost %v (priced %v), want %v", got[x.q].Provider, w.Name, h.Cost, h.Priced, x.cost)
		}
	}
	// and where it isn't, why, for the card's "not enough data" (#2045):
	// money aside and a card with an error say nothing
	for _, x := range []struct {
		q, w int
		why  string
	}{{0, 3, ""}, {1, 1, provider.HoldsLow}, {2, 1, provider.HoldsNoClock}, {3, 0, ""}} {
		w := got[x.q].Windows[x.w]
		if w.Holds != nil || w.HoldsWhy != x.why {
			t.Errorf("%s %s: reckoned %+v, why %q; want none, why %q", got[x.q].Provider, w.Name, w.Holds, w.HoldsWhy, x.why)
		}
	}
	for _, x := range want {
		if why := got[x.q].Windows[x.w].HoldsWhy; why != "" {
			t.Errorf("%s %s: reckoned, and why not %q", got[x.q].Provider, got[x.q].Windows[x.w].Name, why)
		}
	}
	// with no window that can be reckoned, each still says why
	low := []provider.SubscriptionQuota{{Provider: "codex", User: "sam@example.com", Windows: []provider.QuotaWindow{
		{Name: "7 days", Used: 4, ResetsAt: at(4 * 24 * time.Hour), Span: 7 * 24 * time.Hour}}}}
	if w := WithWindowHolds(low, now)[0].Windows[0]; w.Holds != nil || w.HoldsWhy != provider.HoldsLow || low[0].Windows[0].HoldsWhy != "" {
		t.Errorf("4%% used, none reckonable: %+v why %q (the caller's %q), want why %q", w.Holds, w.HoldsWhy, low[0].Windows[0].HoldsWhy, provider.HoldsLow)
	}
	// the cards the caller passed, which a cache may share, are as they were
	for _, q := range qs {
		for _, w := range q.Windows {
			if w.Holds != nil || w.HoldsWhy != "" {
				t.Fatalf("the caller's %s %s was written to", q.Provider, w.Name)
			}
		}
	}
}

// A window begun before magpie's log has calls magpie never saw: its whole
// isn't reckoned, nor one of an account magpie routed nothing for in it.
func TestWindowHoldsNeedsTheWholeWindowLogged(t *testing.T) {
	now := time.Date(2026, 10, 8, 12, 0, 0, 0, time.UTC)
	reset := now.Add(3 * 24 * time.Hour)
	qs := []provider.SubscriptionQuota{{Provider: "claude", User: "dee@example.com", Windows: []provider.QuotaWindow{
		{Name: "7 days", Used: 40, ResetsAt: &reset, Span: 7 * 24 * time.Hour}}}}
	recs := []Record{{Time: now.Add(-time.Hour), Provider: "claude", ProviderAccount: "dee@example.com", Model: "claude-opus-4-7", Input: 100, Output: 100, Status: 200}}
	read := func(fn func(Record)) {
		for _, r := range recs {
			fn(r)
		}
	}
	priceOf := func(Record) *catalog.Price { return nil }
	if got := windowHolds(qs, now, now.Add(-2*24*time.Hour), priceOf, nil, nil, read); got[0].Windows[0].Holds != nil || got[0].Windows[0].HoldsWhy != provider.HoldsBeforeLog {
		t.Fatalf("the log began 2 days into the week, and the week was reckoned: %+v, why %q", got[0].Windows[0].Holds, got[0].Windows[0].HoldsWhy)
	}
	got := windowHolds(qs, now, now.Add(-30*24*time.Hour), priceOf, nil, nil, read)
	if h := got[0].Windows[0].Holds; h == nil || h.Tokens != 500 || h.Priced {
		t.Fatalf("logged since before the week: %+v, want 500 tokens, unpriced", h)
	}
	if got := windowHolds(qs, now, now.Add(-30*24*time.Hour), priceOf, nil, nil, func(func(Record)) {}); got[0].Windows[0].Holds != nil || got[0].Windows[0].HoldsWhy != provider.HoldsNoneRouted {
		t.Fatalf("nothing routed, and the week was reckoned: %+v, why %q", got[0].Windows[0].Holds, got[0].Windows[0].HoldsWhy)
	}
	// a provider since renamed: its calls under the old id are the account's
	recs[0].Provider = "claude-old"
	if got := windowHolds(qs, now, now.Add(-30*24*time.Hour), priceOf, map[string]string{"claude-old": "claude"}, nil, read); got[0].Windows[0].Holds == nil {
		t.Fatal("a call under the provider's old id wasn't counted")
	}
}

// A month's window began on the same day a month before its reset, not 30
// days before: Kiro's credits reset on the 1st.
func TestWindowBoundsOfAMonth(t *testing.T) {
	reset := time.Date(2026, 11, 1, 0, 0, 0, 0, time.UTC)
	w := provider.QuotaWindow{Name: "Credits", Used: 30, ResetsAt: &reset, Span: 30 * 24 * time.Hour}
	start, end, ok := w.Bounds(reset.Add(-time.Hour))
	if !ok || !start.Equal(time.Date(2026, 10, 1, 0, 0, 0, 0, time.UTC)) || !end.Equal(reset) {
		t.Fatalf("bounds %v – %v (%v), want Oct 1 – Nov 1", start, end, ok)
	}
}

// #2010 (NorethSea): once a Codex account's window read none left, the
// calls ChatGPT still answered were paid from credits, and with the
// credits since spent the card has no Balance to set it aside. A window
// with such a call in it isn't reckoned: not the 5 hours that ran dry,
// nor the week the same calls fell in. Calls in a dry run that the vendor
// turned away leave it reckoned, and so does a run before the window began.
func TestWindowHoldsLeavesOutCallsPaidOtherwise(t *testing.T) {
	now := time.Date(2026, 10, 14, 12, 0, 0, 0, time.UTC)
	readAt := now.Add(-time.Minute)
	at := func(d time.Duration) *time.Time { x := now.Add(d); return &x }
	card := func(fiveUsed float64, fiveReset time.Duration) []provider.SubscriptionQuota {
		return []provider.SubscriptionQuota{{Provider: "codex", User: "sample@example.com", ReadAt: &readAt, Windows: []provider.QuotaWindow{
			{Name: "5 hours", Used: fiveUsed, ResetsAt: at(fiveReset), Span: 5 * time.Hour},
			{Name: "7 days", Used: 50, ResetsAt: at(3 * 24 * time.Hour), Span: 7 * 24 * time.Hour},
		}}}
	}
	call := func(ago time.Duration, in int) Record {
		return Record{Time: now.Add(-ago), Agent: "codex", Provider: "codex", ProviderAccount: "sample@example.com", Model: "gpt-5.5", Input: in, Status: 200}
	}
	subscription, credits := call(2*time.Hour, 100000), call(time.Hour, 900000)
	reads := func(recs ...Record) func(func(Record)) {
		return func(fn func(Record)) {
			for _, r := range recs {
				fn(r)
			}
		}
	}
	priceOf := func(Record) *catalog.Price { return nil }
	first := now.Add(-30 * 24 * time.Hour)
	hist := func(five ...provider.QuotaPoint) []provider.QuotaHistory {
		return []provider.QuotaHistory{{Provider: "codex", User: "sample@example.com", Lines: []provider.QuotaLine{{Name: "5 hours", Points: five}}}}
	}
	pt := func(ago time.Duration, left float64) provider.QuotaPoint {
		return provider.QuotaPoint{At: now.Add(-ago), Left: left}
	}
	var whys [2]string
	holds := func(qs []provider.SubscriptionQuota, h []provider.QuotaHistory, recs ...Record) (five, week *provider.WindowHolds) {
		got := windowHolds(qs, now, first, priceOf, nil, dryRuns(qs, h, now), reads(recs...))
		whys = [2]string{got[0].Windows[0].HoldsWhy, got[0].Windows[1].HoldsWhy}
		return got[0].Windows[0].Holds, got[0].Windows[1].Holds
	}

	// the report's case: the 5 hours at 100%, no reading of when it ran dry
	if five, week := holds(card(100, time.Hour), nil, subscription, credits); five != nil || week != nil {
		t.Fatalf("a window ran dry at an unknown time and was reckoned: 5 hours %+v, week %+v", five, week)
	}
	if whys != [2]string{provider.HoldsCredits, provider.HoldsCredits} {
		t.Fatalf("why not reckoned: %q, want credits for both (#2045)", whys)
	}
	// read dry 90 minutes ago, some left 3 hours ago: the call an hour ago was credits
	dry := hist(pt(3*time.Hour, 40), pt(90*time.Minute, 0))
	if five, week := holds(card(100, time.Hour), dry, subscription, credits); five != nil || week != nil {
		t.Fatalf("a call answered after the 5 hours read none left was reckoned: 5 hours %+v, week %+v", five, week)
	}
	// nothing answered after it ran dry (no credits: turned away): the calls were the windows'
	five, week := holds(card(100, time.Hour), dry, subscription)
	if five == nil || five.Routed.Tokens != 100000 || week == nil || week.Tokens != 200000 {
		t.Fatalf("no call paid otherwise: 5 hours %+v, week %+v, want 100000 and 200000", five, week)
	}
	// a run dry earlier in the week, refilled since: the 5 hours since are
	// reckoned, the week holding its credits call isn't
	earlier := call(7*time.Hour, 900000)
	refilled := hist(pt(10*time.Hour, 40), pt(8*time.Hour, 0), pt(6*time.Hour, 100), pt(90*time.Minute, 80))
	five, week = holds(card(20, 3*time.Hour), refilled, subscription, earlier)
	if five == nil || five.Routed.Tokens != 100000 || week != nil {
		t.Fatalf("after a dry run refilled: 5 hours %+v (want 100000 routed), week %+v (want none)", five, week)
	}
	// the same run before the week began leaves the week reckoned
	before := hist(pt(9*24*time.Hour, 40), pt(8*24*time.Hour, 0), pt(8*24*time.Hour-time.Hour, 100))
	if _, week = holds(card(20, 3*time.Hour), before, subscription, call(8*24*time.Hour-30*time.Minute, 900000)); week == nil || week.Routed.Tokens != 100000 {
		t.Fatalf("a dry run before the week: week %+v, want 100000 routed", week)
	}
}

// A successful call after the vendor's reset is in the new allowance,
// even when the first reading of the refill arrives a few seconds later.
func TestWindowHoldsResetBeforeRefillReading(t *testing.T) {
	now := time.Date(2026, 10, 14, 12, 0, 0, 0, time.UTC)
	reset := now.Add(-time.Hour)
	refill := reset.Add(3 * time.Second)
	lastLeft, firstDry := reset.Add(-90*time.Minute), reset.Add(-87*time.Minute)
	weekReset, fiveReset := now.Add(3*24*time.Hour), reset.Add(5*time.Hour)
	for _, providerID := range []string{"codex", "claude"} {
		t.Run(providerID, func(t *testing.T) {
			qs := []provider.SubscriptionQuota{{Provider: providerID, User: "sample@example.com", ReadAt: &now, Windows: []provider.QuotaWindow{
				{Name: "5 hours", Used: 20, ResetsAt: &fiveReset, Span: 5 * time.Hour},
				{Name: "7 days", Used: 50, ResetsAt: &weekReset, Span: 7 * 24 * time.Hour},
			}}}
			hist := []provider.QuotaHistory{{Provider: providerID, User: "sample@example.com", Lines: []provider.QuotaLine{{Name: "5 hours", Points: []provider.QuotaPoint{
				{At: lastLeft, Left: 3, ResetsAt: &reset}, {At: firstDry, Left: 0, ResetsAt: &reset}, {At: refill, Left: 100, ResetsAt: &fiveReset},
			}}}}}
			for _, tc := range []struct {
				name string
				at   time.Time
				want bool
			}{
				{"just after reset", reset.Add(585 * time.Millisecond), true},
				{"at reset", reset, true},
				{"before reset", reset.Add(-time.Second), false},
			} {
				t.Run(tc.name, func(t *testing.T) {
					call := Record{Time: tc.at, Provider: providerID, ProviderAccount: "sample@example.com", Model: "m", Input: 1000, Status: 200}
					got := windowHolds(qs, now, now.Add(-30*24*time.Hour), func(Record) *catalog.Price { return nil }, nil, dryRuns(qs, hist, now), func(fn func(Record)) { fn(call) })
					h := got[0].Windows[1].Holds
					if (h != nil) != tc.want {
						t.Fatalf("week holds = %+v, want present %v", h, tc.want)
					}
				})
			}
		})
	}
}

func TestDryRunsKeepsTheCurrentReadingReset(t *testing.T) {
	now := time.Date(2026, 10, 14, 12, 0, 0, 0, time.UTC)
	qs := []provider.SubscriptionQuota{{Provider: "plugin", User: "sample", Windows: []provider.QuotaWindow{{Name: "5 hours", Used: 100, ResetSecs: 3600}}}}
	runs := dryRuns(qs, nil, now)["plugin\x00sample"]
	if len(runs) != 1 || runs[0].holds(now.Add(time.Hour)) || !runs[0].holds(now.Add(time.Hour-time.Second)) {
		t.Fatalf("current relative reset: %+v", runs)
	}
}

// A window that ran dry, reset, and ran dry again before its next reset
// hides only the calls inside each dry stretch: the 7-day estimate still
// shows when its calls fall outside them (#2010).
func TestWindowHoldsDryAgainAfterReset(t *testing.T) {
	at := func(t time.Time) *time.Time { return &t }
	now := time.Date(2026, 10, 14, 12, 0, 0, 0, time.UTC)
	r1 := now.Add(-6 * time.Hour)
	r2 := r1.Add(5 * time.Hour)
	weekReset := now.Add(3 * 24 * time.Hour)
	qs := []provider.SubscriptionQuota{{Provider: "codex", User: "u", ReadAt: &now, Windows: []provider.QuotaWindow{
		{Name: "5 hours", Used: 20, ResetsAt: at(now.Add(4 * time.Hour)), Span: 5 * time.Hour},
		{Name: "7 days", Used: 50, ResetsAt: &weekReset, Span: 7 * 24 * time.Hour},
	}}}
	hist := []provider.QuotaHistory{{Provider: "codex", User: "u", Lines: []provider.QuotaLine{{Name: "5 hours", Points: []provider.QuotaPoint{
		{At: r1.Add(-2 * time.Hour), Left: 10, ResetsAt: &r1}, {At: r1.Add(-time.Hour), Left: 0, ResetsAt: &r1},
		{At: r1.Add(2 * time.Hour), Left: 0, ResetsAt: &r2}, {At: r2.Add(time.Minute), Left: 100, ResetsAt: at(r2.Add(5 * time.Hour))},
	}}}}}
	calls := []Record{
		{Time: now.Add(-3 * 24 * time.Hour), Input: 1000, Status: 200},
		{Time: r1.Add(time.Hour), Input: 1000, Status: 200}, // after the reset, before the next dry reading
	}
	got := windowHolds(qs, now, now.Add(-30*24*time.Hour), func(Record) *catalog.Price { return nil }, nil, dryRuns(qs, hist, now), func(fn func(Record)) {
		for _, c := range calls {
			c.Provider, c.ProviderAccount, c.Model = "codex", "u", "m"
			fn(c)
		}
	})
	if w := got[0].Windows[1]; w.Holds == nil || w.Holds.Tokens != 4000 {
		t.Fatalf("7 days: %+v (%q), want 4000 tokens", w.Holds, w.HoldsWhy)
	}
}

// #2333: magpie quota --json and GET /v1/magpie/quotas reckon a window's
// whole by the same WithWindowHolds the Usage page does, which usage hands
// provider (ReckonHolds) since provider can't read the log itself.
func TestQuotaReportsReckonByTheLog(t *testing.T) {
	holdsHome(t)
	now := holdClock(t, time.Date(2026, 10, 8, 12, 0, 0, 0, time.UTC))
	reset := now.Add(2 * time.Hour)
	for _, ago := range []time.Duration{8 * 24 * time.Hour, time.Hour} {
		Append(Record{Time: now.Add(-ago), Agent: "codex", Provider: "codex", ProviderAccount: "sam@example.com", Model: "gpt-5.5", Input: 4000, Output: 1000, Status: 200})
	}
	if provider.ReckonHolds == nil {
		t.Fatal("provider.ReckonHolds isn't set: the quota reports tell no whole")
	}
	qs := provider.ReckonHolds([]provider.SubscriptionQuota{{Provider: "codex", User: "sam@example.com", Windows: []provider.QuotaWindow{
		{Name: "5 hours", Used: 10, ResetsAt: &reset, Span: 5 * time.Hour},
		{Name: "7 days", Used: 2, ResetsAt: &reset, Span: 7 * 24 * time.Hour},
	}}}, now)
	if h := qs[0].Windows[0].Holds; h == nil || h.Tokens != 50000 || !h.Priced {
		t.Fatalf("5 hours: %+v, want 50000 tokens, priced", h)
	}
	if qs[0].Windows[1].HoldsWhy != provider.HoldsLow {
		t.Fatalf("7 days: why %q, want %q", qs[0].Windows[1].HoldsWhy, provider.HoldsLow)
	}
}

// #2344: a window's whole is priced like the Usage page's cost, so a call
// to a provider's own id no list prices is priced at the maker's list
// price of the model the user said it is the same as (ModelSameAs), never
// at a rate; without one it stays unpriced.
func TestHoldsPriceTheModelItIsTheSameAs(t *testing.T) {
	holdsHome(t)
	if err := provider.Save(provider.Provider{ID: "wb", Name: "WB", Key: "k", Chat: "https://wb.example/v1",
		Models: []string{"gpt-5-5-wb", "other"}, PriceRate: 0.5}); err != nil {
		t.Fatal(err)
	}
	s := settings.Load()
	s.ModelSameAs = map[string]string{"wb/gpt-5-5-wb": "gpt-5.5"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	priceOf := apiPricer()
	if pr := priceOf(Record{Provider: "wb", Model: "gpt-5-5-wb"}); pr == nil || pr.Input != 1.25 {
		t.Errorf("the model it is the same as wasn't its API price: %+v", pr)
	}
	if pr := priceOf(Record{Provider: "wb", Model: "other"}); pr != nil {
		t.Errorf("a model with no Same as was priced: %+v", pr)
	}
}
