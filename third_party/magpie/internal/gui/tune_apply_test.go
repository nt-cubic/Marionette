package gui

import (
	"encoding/json"
	"strconv"
	"testing"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/codexcat"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
)

// hoobnn on Discord (11-01): under Usage › Context, over 7 or 30 days,
// Use on Codex's window did nothing. Those ranges held calls on a model
// whose provider has a Compact at of its own (430K), which Codex runs
// to whatever Long conversations says; today's calls didn't. "Now" was
// the largest window over every model, that one's 430K, so a Use put
// the advice in Long conversations, moved Codex's other models to it,
// and the card still read 430K, now held as that model's own. "Now" is
// where Long conversations has Codex's other models; a model with its
// own threshold is named apart. With no other model in the range, the
// card is held before the click as well, Long conversations at magpie's
// default or not, since Use couldn't move it.
func TestTuneCodexUseLandsInEveryRange(t *testing.T) {
	sandboxHome(t)
	home := t.TempDir()
	for _, p := range []provider.Provider{
		{ID: "cpa", Name: "CPA", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-cpa-1m"}, Contexts: map[string]int{"gpt-cpa-1m": 1000000}},
		{ID: "own", Name: "Own", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"gpt-own-1m"}, Contexts: map[string]int{"gpt-own-1m": 1000000}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SetModelCompacts("own", map[string]int{"*": 430000}); err != nil {
		t.Fatal(err)
	}
	advice := func(models ...string) tuneAdvice {
		t.Helper()
		var cs []sessions.Call
		for _, m := range models {
			cs = append(cs, tuneCalls("codex", m, 80)...)
		}
		st := tuneOf(cs, 7, home)
		if len(st.Agents) != 1 || len(st.Agents[0].Advice) != 1 || st.Agents[0].Advice[0].Compact == nil {
			t.Fatalf("%v: no compaction advice for Codex: %+v", models, st)
		}
		return st.Agents[0].Advice[0]
	}
	// where Codex's model list, as magpie gives it, has Codex compact
	landed := func() map[string]int {
		t.Helper()
		var out struct {
			Models []struct {
				Slug string `json:"slug"`
				At   *int   `json:"auto_compact_token_limit"`
			} `json:"models"`
		}
		if err := json.Unmarshal(codexcat.Catalog(provider.CodexCatalog(provider.Catalog())), &out); err != nil {
			t.Fatal(err)
		}
		at := map[string]int{}
		for _, m := range out.Models {
			if m.At != nil {
				at[m.Slug] = *m.At
			}
		}
		return at
	}

	// one model, with a threshold of its own, and Long conversations at
	// magpie's default: Use couldn't move it, so there is none
	if a := advice("own/gpt-own-1m"); a.Own == nil || a.Own.Model != "own/gpt-own-1m" || a.Own.Value != 430000 || a.Compact.Current != 430000 {
		t.Errorf("only the model with its own 430K: own %+v, now %d; want it named, now 430000", a.Own, a.Compact.Current)
	}

	week := advice("cpa/gpt-cpa-1m", "own/gpt-own-1m")
	if week.Own != nil {
		t.Fatalf("7 days: held as %+v, though Long conversations moves cpa/gpt-cpa-1m", week.Own)
	}
	if len(week.Apart) != 1 || week.Apart[0] != (tuneOwn{Model: "own/gpt-own-1m", Value: 430000}) {
		t.Errorf("7 days: apart %+v; want own/gpt-own-1m at 430000", week.Apart)
	}
	if today := advice("cpa/gpt-cpa-1m"); today.Compact.Current != week.Compact.Current || len(today.Apart) != 0 {
		t.Errorf("Codex now: %d today, %d over 7 days (apart %+v); want the same", today.Compact.Current, week.Compact.Current, today.Apart)
	}
	best := week.Compact.Best
	if best == week.Compact.Current {
		t.Fatalf("the advice is %d, where Codex runs now: the case needs one that changes it", best)
	}

	// the click, as /api/tune/apply makes it
	if err := agent.TuneApply(home, "codex", agent.TuneCompact, "", strconv.Itoa(best)); err != nil {
		t.Fatal(err)
	}
	for _, ms := range [][]string{{"cpa/gpt-cpa-1m"}, {"cpa/gpt-cpa-1m", "own/gpt-own-1m"}} {
		a := advice(ms...)
		if a.Compact.Current != best || a.Own != nil || !a.Setting.Ours || a.Setting.Value != strconv.Itoa(best) {
			t.Errorf("%v after Use %d: now %d, own %+v, setting %+v; want now %d, put in by magpie", ms, best, a.Compact.Current, a.Own, a.Setting, best)
		}
	}
	at := landed()
	if at["cpa/gpt-cpa-1m"] != best || at["own/gpt-own-1m"] != 430000 {
		t.Errorf("Codex's list after Use %d: compacts at %v; want cpa/gpt-cpa-1m at %d, own/gpt-own-1m at its own 430000", best, at, best)
	}
}
