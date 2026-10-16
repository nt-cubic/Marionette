package gui

import (
	"fmt"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
)

// tuneCalls is an agent's conversations on one model that grow by 5K a
// call from 20K, n calls each, and are never compacted: at 80 the largest
// prompt magpie has seen is 415K, whatever the model's window.
func tuneCalls(agent, model string, n int) []sessions.Call {
	var out []sessions.Call
	t0 := time.Now().Add(-48 * time.Hour)
	for s := range 2 {
		for i := range n {
			c := sessions.Call{Time: t0.Add(time.Duration(s)*time.Hour + time.Duration(i)*20*time.Second), Agent: agent,
				Session: fmt.Sprint("s", s), Model: model, File: fmt.Sprintf("/sessions/%s-%d.jsonl", agent, s)}
			c.Input, c.Output = 20000+5000*i, 800
			out = append(out, c)
		}
	}
	return out
}

// The windows Tuned for you offers Codex for a model (zhemu on Discord: a
// CLIProxyAPI provider's GPT model with 1M, Settings › Long conversations
// at 750K, offered no more than 412K, the largest prompt it had been sent).
// A model whose window magpie doesn't know is offered up to 1M, not capped
// under the user's 750K; one typed at 1M is offered to 1M and runs now at
// the 750K magpie tells Codex; one typed at 400K is capped at its window.
// Claude Code is offered no window for another vendor's model, whose
// window is magpie's own (Settings), and a Claude model keeps its 1M.
func TestTuneWindowsAreTheUsers(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	home := t.TempDir()
	if err := provider.Save(provider.Provider{ID: "cpa", Name: "CPA", Key: "k", Chat: "http://127.0.0.1:1/v1",
		Models:   []string{"gpt-cpa-open", "gpt-cpa-1m", "gpt-cpa-400k"},
		Contexts: map[string]int{"gpt-cpa-1m": 1000000, "gpt-cpa-400k": 400000}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetCompactAt(750000); err != nil {
		t.Fatal(err)
	}
	for _, e := range provider.Catalog() {
		if e.ID == "cpa/gpt-cpa-open" && e.Context != 0 {
			t.Fatalf("cpa/gpt-cpa-open has a window of %d: the case needs one magpie doesn't know", e.Context)
		}
	}
	codex := func(model string) tuneAdvice {
		t.Helper()
		n := 80
		if model == "cpa/gpt-cpa-400k" {
			n = 70 // to 365K, inside its window
		}
		st := tuneOf(tuneCalls("codex", model, n), 7, home)
		if len(st.Agents) != 1 || len(st.Agents[0].Advice) != 1 || st.Agents[0].Advice[0].Compact == nil {
			t.Fatalf("%s: no compaction advice for Codex: %+v", model, st)
		}
		return st.Agents[0].Advice[0]
	}
	top := func(a tuneAdvice) int { return a.Compact.Curve[len(a.Compact.Curve)-1].Value }

	for _, c := range []struct {
		model             string
		top, window, curr int
	}{
		{"cpa/gpt-cpa-open", 1000000, 0, 1000000},
		{"cpa/gpt-cpa-1m", 1000000, 1000000, 750000},
		{"cpa/gpt-cpa-400k", 400000, 400000, 400000},
	} {
		a := codex(c.model)
		if got := top(a); got != c.top {
			t.Errorf("Codex on %s: windows offered up to %d; want %d", c.model, got, c.top)
		}
		if a.Compact.Window != c.window || a.Compact.Current != c.curr {
			t.Errorf("Codex on %s: window %d, now %d; want window %d, now %d", c.model, a.Compact.Window, a.Compact.Current, c.window, c.curr)
		}
	}

	// calls on the 400K model and on the one with no known window: that
	// one isn't capped by the other's 400K, and what Codex runs at now
	// isn't known for both
	both := tuneOf(append(tuneCalls("codex", "cpa/gpt-cpa-400k", 70), tuneCalls("codex", "cpa/gpt-cpa-open", 80)...), 7, home)
	if a := both.Agents[0].Advice[0]; top(a) != 1000000 || a.Compact.Window != 0 {
		t.Errorf("Codex on two models, one window unknown: offered up to %d, window %d; want 1M, unknown", top(a), a.Compact.Window)
	}

	for _, m := range []string{"cpa/gpt-cpa-open", "cpa/gpt-cpa-1m"} {
		st := tuneOf(tuneCalls("claude", m, 80), 7, home)
		for _, ag := range st.Agents {
			for _, a := range ag.Advice {
				if a.Compact != nil {
					t.Errorf("Claude Code on %s is offered windows %+v; its window is magpie's own", m, a.Compact.Curve)
				}
			}
		}
	}
	st := tuneOf(tuneCalls("claude", "claude-opus-4-6", 80), 7, home)
	if len(st.Agents) != 1 || len(st.Agents[0].Advice) == 0 || st.Agents[0].Advice[0].Compact == nil {
		t.Fatalf("no compaction advice for Claude Code on claude-opus-4-6: %+v", st)
	}
	if a := st.Agents[0].Advice[0]; top(a) != 1000000 || a.Compact.Window != 1000000 {
		t.Errorf("Claude Code on claude-opus-4-6, past 200K: offered up to %d of %d; want 1M", top(a), a.Compact.Window)
	}
}
