package gui

import (
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Where Tuned for you says Codex runs now is worked out the same way in
// every range (hoobnn on Discord). Over 7 days the calls held one of
// Codex's own models as well, outside magpie's catalog, which made "Now"
// unknown for the whole range: it was guessed from the prompts (380K)
// while today's calls, on magpie's models alone, read the 430K magpie
// tells Codex. And a model's own Compact at is where Codex runs, over Long
// conversations: the card says so instead of "In use" for a Long
// conversations value that doesn't move it.
func TestTuneCodexNowIsTheSameInEveryRange(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	home := t.TempDir()
	if err := provider.Save(provider.Provider{ID: "cpa", Name: "CPA", Key: "k", Chat: "http://127.0.0.1:1/v1",
		Models:   []string{"gpt-cpa-1m", "gpt-cpa-own"},
		Contexts: map[string]int{"gpt-cpa-1m": 1000000, "gpt-cpa-own": 440000}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetCompactAt(170000); err != nil {
		t.Fatal(err)
	}
	advice := func(models ...string) tuneAdvice {
		t.Helper()
		var cs = tuneCalls("codex", models[0], 80)
		for _, m := range models[1:] {
			cs = append(cs, tuneCalls("codex", m, 80)...)
		}
		st := tuneOf(cs, 7, home)
		if len(st.Agents) != 1 || len(st.Agents[0].Advice) != 1 || st.Agents[0].Advice[0].Compact == nil {
			t.Fatalf("%v: no compaction advice for Codex: %+v", models, st)
		}
		return st.Agents[0].Advice[0]
	}

	// Long conversations at 170K: magpie's model runs at 170K, with one of
	// Codex's own models beside it or not
	today, week := advice("cpa/gpt-cpa-1m"), advice("cpa/gpt-cpa-1m", "gpt-5.5")
	if today.Compact.Current != 170000 || week.Compact.Current != 170000 {
		t.Errorf("Codex now: %d on magpie's model, %d with Codex's own beside it; want 170000 both", today.Compact.Current, week.Compact.Current)
	}
	if today.Own != nil || week.Own != nil {
		t.Errorf("Long conversations is where Codex runs, yet the advice names a model's own threshold: %+v, %+v", today.Own, week.Own)
	}

	// the provider's own Compact at, 430K, over Long conversations' 170K
	if err := provider.SetModelCompacts("cpa", map[string]int{"*": 430000}); err != nil {
		t.Fatal(err)
	}
	for _, ms := range [][]string{{"cpa/gpt-cpa-own"}, {"cpa/gpt-cpa-own", "gpt-5.5"}} {
		a := advice(ms...)
		if a.Compact.Current != 430000 {
			t.Errorf("%v: Codex now %d; want the provider's 430000", ms, a.Compact.Current)
		}
		if a.Own == nil || a.Own.Model != "cpa/gpt-cpa-own" || a.Own.Value != 430000 {
			t.Errorf("%v: own threshold %+v; want cpa/gpt-cpa-own at 430000", ms, a.Own)
		}
	}
}
