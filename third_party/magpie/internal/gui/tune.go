package gui

// The Context tab's advice: for each agent, the settings that would have
// cost this user the fewest tokens over the period, worked out by
// replaying their own calls (internal/tune) against what the agent's
// config has now, and putting one in, or back, on their say.

import (
	"encoding/json"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/tune"
)

type tuneAdvice struct {
	Knob    string            `json:"knob"`
	Model   string            `json:"model,omitempty"`
	Setting agent.TuneSetting `json:"setting"`
	Compact *tune.Compaction  `json:"compact,omitempty"`
	TTL     *tune.TTL         `json:"ttl,omitempty"`
}

type tuneAgent struct {
	Agent  string       `json:"agent"`
	Calls  int          `json:"calls"`
	Advice []tuneAdvice `json:"advice"`
}

type tuneState struct {
	Days   int         `json:"days"`
	Agents []tuneAgent `json:"agents"`
}

// tuneOf works the advice out of the calls the agents' files hold.
func tuneOf(cs []sessions.Call, days int, home string) tuneState {
	by := map[string][]tune.Call{}
	for _, c := range cs {
		if c.Agent == "claude" || c.Agent == "codex" {
			by[c.Agent] = append(by[c.Agent], tune.FromCalls([]sessions.Call{c})...)
		}
	}
	out := tuneState{Days: days, Agents: []tuneAgent{}}
	for _, name := range []string{"claude", "codex"} {
		calls := by[name]
		if len(calls) == 0 {
			continue
		}
		ta := tuneAgent{Agent: name, Calls: len(calls), Advice: []tuneAdvice{}}
		switch name {
		case "claude":
			models := map[string][]tune.Call{}
			var main, side []tune.Call
			for _, c := range calls {
				models[c.Model] = append(models[c.Model], c)
				if c.Sub {
					side = append(side, c)
				} else {
					main = append(main, c)
				}
			}
			for m, mc := range models {
				s, err := agent.TuneRead(home, name, agent.TuneCompact, m)
				if err != nil {
					continue
				}
				// a model asked for with [1m], or seen past 200K, has the
				// whole 1M; another the 200K Claude Code's own models start with
				window := 200000
				for _, c := range mc {
					if c.Long || c.Prompt > window {
						window = 1000000
						break
					}
				}
				cur, _ := strconv.Atoi(s.Value)
				o := tune.CompactOpts{Min: 100000, Max: 1000000, Step: 10000, Reserve: 33000, Window: window, Current: cur}
				if r := tune.Compact(mc, o); r != nil {
					ta.Advice = append(ta.Advice, tuneAdvice{Knob: agent.TuneCompact, Model: m, Setting: s, Compact: r})
				}
			}
			sort.SliceStable(ta.Advice, func(i, j int) bool {
				return ta.Advice[i].Compact.Facts.Calls > ta.Advice[j].Compact.Facts.Calls
			})
			for _, k := range []struct {
				knob  string
				calls []tune.Call
			}{{agent.TuneCache, main}, {agent.TuneSubCache, side}} {
				s, err := agent.TuneRead(home, name, k.knob, "")
				if err != nil {
					continue
				}
				if r := tune.CacheTTL(k.calls, s.Value); r != nil {
					ta.Advice = append(ta.Advice, tuneAdvice{Knob: k.knob, Setting: s, TTL: r})
				}
			}
		case "codex":
			s, err := agent.TuneRead(home, name, agent.TuneCompact, "")
			if err != nil {
				continue
			}
			st := settings.Load()
			window, told := codexWindow(calls, st, provider.Catalog(), provider.GroupFinder())
			cur, _ := strconv.Atoi(s.Value)
			if cur == 0 {
				cur = told
			}
			// a value the user chose is never past the last one offered
			o := tune.CompactOpts{Min: 50000, Max: max(1000000, window, cur, st.CompactAt), Step: 10000, Window: window, Current: cur}
			if r := tune.Compact(calls, o); r != nil {
				ta.Advice = append(ta.Advice, tuneAdvice{Knob: agent.TuneCompact, Setting: s, Compact: r})
			}
		}
		out.Agents = append(out.Agents, ta)
	}
	return out
}

// codexWindow is the window Codex runs the calls' models to, and where
// magpie has it compact them when its config.toml says nothing (told; 0
// when that isn't known for every model). A model of magpie's catalog
// has the window it is listed with, one the user typed or the vendor's,
// and is told the threshold the user set on it, else Settings › Long
// conversations (provider.CompactFor, as codexcat writes it). One whose
// window magpie doesn't know caps nothing (window 0): the largest prompt
// it had been sent was taken for its window, and a 1M model set to
// compact at 750K was offered no more than 412K (zhemu on Discord).
// Codex's own models, outside magpie's catalog, run to 272K or the
// largest prompt seen, as before.
func codexWindow(calls []tune.Call, st settings.Settings, entries []provider.Entry, find func(string) (provider.Group, []provider.Member, bool)) (window, told int) {
	byID := make(map[string]provider.Entry, len(entries))
	for _, e := range entries {
		byID[strings.ToLower(e.ID)] = e
	}
	peak := map[string]int{}
	for _, c := range calls {
		peak[c.Model] = max(peak[c.Model], c.Prompt)
	}
	unknown, all := false, true
	for m, p := range peak {
		e, ok := byID[strings.TrimPrefix(m, "magpie/")]
		switch {
		case !ok:
			window, all = max(window, 272000, p), false
		case e.Context <= 0:
			unknown, all = true, false
		default:
			window = max(window, e.Context, p)
			at := st.Working(e.Context)
			if n := provider.CompactFor(st, e, find); n > 0 {
				at = min(e.Context, n)
			}
			told = max(told, at)
		}
	}
	if unknown {
		window = 0
	}
	if !all {
		told = 0
	}
	return window, told
}

func tuneRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/tune", func(rw http.ResponseWriter, r *http.Request) {
		days, _ := strconv.Atoi(r.URL.Query().Get("days"))
		if days <= 0 || days > 30 {
			days = 7
		}
		since := time.Now().AddDate(0, 0, -days)
		writeJSON(rw, tuneOf(sessions.Calls(since), days, ""))
	})
	type change struct {
		Agent, Knob, Model, Value string
	}
	mux.HandleFunc("POST /api/tune/apply", func(rw http.ResponseWriter, r *http.Request) {
		var in change
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if err := agent.TuneApply("", in.Agent, in.Knob, in.Model, in.Value); err != nil {
			fail(rw, err)
			return
		}
		s, _ := agent.TuneRead("", in.Agent, in.Knob, in.Model)
		writeJSON(rw, s)
	})
	mux.HandleFunc("POST /api/tune/undo", func(rw http.ResponseWriter, r *http.Request) {
		var in change
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if err := agent.TuneUndo("", in.Agent, in.Knob, in.Model); err != nil {
			fail(rw, err)
			return
		}
		s, _ := agent.TuneRead("", in.Agent, in.Knob, in.Model)
		writeJSON(rw, s)
	})
}
