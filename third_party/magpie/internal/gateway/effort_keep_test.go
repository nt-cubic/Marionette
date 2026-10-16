package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A group whose effort is auto keeps a conversation's reasoning from one
// turn to the next while the vendor's cache of it is warm (#617): a turn
// the decision model takes to be harder goes up, one it takes to be easier
// keeps the higher level (the trace shows the pick it wanted), its tool
// rounds with it, and once the conversation went unanswered past
// cacheCold the easier pick is taken.
func TestAutoEffortKeepsTheCache(t *testing.T) {
	now := time.Date(2026, 10, 3, 17, 27, 0, 0, time.Local)
	ruleClock = func() time.Time { return now }
	t.Cleanup(func() { ruleClock = time.Now })
	s, a, _, j := jevved(t, provider.EffortAuto)
	if err := provider.SetModelEfforts("a/small", []string{"low", "medium", "high", "xhigh"}); err != nil {
		t.Fatal(err)
	}
	var said []string
	turn := func(score float64, words string, tools int, wantPick, wantWanted, wantSent string) {
		t.Helper()
		j.score = score
		if tools == 0 {
			said = append(said, words)
		}
		_, r := postOK(t, s, "pi", chat(said[0], said[1:], tools, `,"reasoning_effort":"medium"`))
		sent := sentBody(t, a)["reasoning_effort"]
		if r.Rule == nil || r.Rule.Pick != wantPick || r.Rule.Wanted != wantWanted || sent != wantSent {
			t.Fatalf("%q (%d tool rounds): pick %+v, sent %v; want pick %q wanted %q sent %q", words, tools, r.Rule, sent, wantPick, wantWanted, wantSent)
		}
	}
	turn(2, "add a retry to the fetcher", 0, "high", "", "high")
	now = now.Add(30 * time.Second)
	turn(3, "now it deadlocks under load, find out why", 0, "xhigh", "", "xhigh") // up: taken
	now = now.Add(30 * time.Second)
	turn(2, "rename the helper", 0, "xhigh", "high", "xhigh") // down: kept for the cache
	now = now.Add(10 * time.Second)
	turn(2, "rename the helper", 2, "xhigh", "high", "xhigh") // its tool rounds too
	now = now.Add(cacheCold + time.Minute)
	turn(1, "and the test name", 0, "medium", "", "medium") // the cache went cold: the pick is taken
}

// Where an effort change goes as a configuration_update (Responses on a
// GPT-6 model through a ChatGPT account, MAGPIE_EFFORT_UPDATES=on), the
// easier pick isn't held back: the update carries it and the top-level
// effort, and the cache with it, stays the thread's first.
func TestAutoEffortGoesAsUpdate(t *testing.T) {
	t.Setenv("MAGPIE_EFFORT_UPDATES", "on")
	f := &fake{t: t, ctype: "none", reply: effortReply}
	effortCodexAccount(t, f)
	j := &jevUp{choice: noIntent, sure: 0.9}
	srv := httptest.NewServer(j)
	t.Cleanup(srv.Close)
	if err := provider.Save(provider.Provider{ID: "ts", Name: "TypeSafe", Key: "kts", Decide: srv.URL + "/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "core", Name: "Core", Members: []string{"codex/gpt-6-luna"}, Effort: provider.EffortAuto, Classifier: "ts/jev-latest"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetModelEfforts("codex/gpt-6-luna", []string{"low", "medium", "high", "xhigh"}); err != nil {
		t.Fatal(err)
	}
	s := New()
	var said []string
	for i, c := range []struct {
		score      float64
		words      string
		wantEffort string
		wantInput  string
		wantWanted string
		wantPicked string
	}{
		{3, "find the deadlock", "xhigh", "user", "", "xhigh"},
		{2, "rename the helper", "xhigh", "user,assistant,u:high,user", "high", "xhigh"},
	} {
		j.score = c.score
		said = append(said, c.words)
		body := strings.Replace(effortTurn("pi", "medium", said...), `"codex/gpt-6-luna"`, `"group/core"`, 1)
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
		if rec.Code != 200 {
			t.Fatalf("turn %d: %d %s", i, rec.Code, rec.Body)
		}
		effort, _, input := sentShape(t, f.got)
		r := lastRoute(s)
		if effort != c.wantEffort || strings.Join(input, ",") != c.wantInput || r.Rule == nil || r.Rule.Pick != c.wantPicked || r.Rule.Wanted != c.wantWanted {
			t.Fatalf("turn %d: sent %q %v, rule %+v", i, effort, input, r.Rule)
		}
	}
}

// keepsEffort keeps a turn before's level only against a lower pick, while
// the cache is warm and the conversation wasn't compacted.
func TestKeepsEffort(t *testing.T) {
	at := time.Date(2026, 10, 3, 19, 13, 0, 0, time.Local)
	tr := turnRule{effort: "xhigh", at: at, size: 230_000}
	for _, c := range []struct {
		name   string
		tr     turnRule
		pick   string
		tokens int
		now    time.Time
		want   bool
	}{
		{"lower, warm", tr, "high", 234_000, at.Add(33 * time.Second), true},
		{"higher", turnRule{effort: "medium", at: at}, "high", 1000, at.Add(time.Second), false},
		{"same", tr, "xhigh", 234_000, at.Add(time.Second), false},
		{"cold", tr, "high", 234_000, at.Add(cacheCold + time.Second), false},
		{"answered since: warm", turnRule{effort: "xhigh", at: at, answered: at.Add(4 * time.Minute)}, "low", 1000, at.Add(6 * time.Minute), true},
		{"compacted", tr, "high", 40_000, at.Add(time.Second), false},
		{"an hour's cache: warm at 20 minutes", turnRule{effort: "xhigh", at: at, hour: hourCache{30_000, at}}, "high", 30_000, at.Add(20 * time.Minute), true},
		{"an hour's cache: cold after the hour", turnRule{effort: "xhigh", at: at, hour: hourCache{30_000, at}}, "high", 30_000, at.Add(61 * time.Minute), false},
		{"nothing before", turnRule{at: at}, "high", 1000, at.Add(time.Second), false},
		{"no pick", tr, "", 234_000, at.Add(time.Second), false},
	} {
		if got := keepsEffort(c.tr, c.pick, c.tokens, c.now); got != c.want {
			t.Errorf("%s: %v, want %v", c.name, got, c.want)
		}
	}
}
