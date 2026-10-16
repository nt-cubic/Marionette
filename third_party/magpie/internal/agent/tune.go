package agent

// The settings tune advises (internal/tune), as each agent's config has
// them, and putting one in on the user's say: Claude Code's auto-compact
// window for a model and its cache lifetimes, Codex's auto-compact limit.
// What was there before is stashed, so Undo puts it back; a variable that
// takes precedence over the setting is told of, never written over.

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/yetone/magpie/internal/edit"
)

// The settings tune knows.
const (
	TuneCompact  = "compact"            // when the agent compacts its conversation, in tokens
	TuneCache    = "cache_ttl"          // Claude Code: the main conversation's cache, 5m or 1h
	TuneSubCache = "subagent_cache_ttl" // Claude Code: its subagents', workflows' and helpers'
)

// TuneSetting is one of those settings as an agent has it now.
type TuneSetting struct {
	Value string `json:"value,omitempty"` // "" when nothing sets it
	// Where Value comes from: settings (the agent's own config file),
	// model (Claude Code's modelSettings for the model), env
	Source string `json:"source,omitempty"`
	// Locked names the variable that takes precedence over the setting,
	// which tune leaves as it is: putting the advice in would change nothing
	Locked string `json:"locked,omitempty"`
	// Ours: tune put Value in, and Undo puts back what was there
	Ours bool `json:"ours,omitempty"`
}

// tuneStash is what a setting was before tune put one in.
type tuneStash struct {
	Had bool   `json:"had"`
	Raw string `json:"raw,omitempty"` // as the file had it, JSON or TOML
	Set string `json:"set"`           // the value tune put in
}

func tuneKey(agent, knob, model string) string {
	k := "tune." + agent + "." + knob
	if model != "" {
		k += "." + model
	}
	return k
}

func tuneHome(home string) string {
	if home != "" {
		return home
	}
	h, _ := os.UserHomeDir()
	return h
}

// tunePlace is where a setting lives: the file, its key path, and the
// variables that take precedence over it, first first.
type tunePlace struct {
	path   string
	key    string
	toml   bool
	env    []string
	number bool
}

func tuneWhere(home, agent, knob, model string) (tunePlace, error) {
	home = tuneHome(home)
	switch agent {
	case "claude":
		path := filepath.Join(here(home).claudeDir(), "settings.json")
		switch knob {
		case TuneCompact:
			n := claudeName(model)
			if n == "" {
				return tunePlace{}, fmt.Errorf("%q is no Claude model: Claude Code's window for it is magpie's own, under Settings", model)
			}
			return tunePlace{path: path, key: "modelSettings." + n + ".autoCompactWindow", env: []string{claudeCompactEnv}, number: true}, nil
		case TuneCache:
			return tunePlace{path: path, key: "promptCacheTtl", env: []string{"FORCE_PROMPT_CACHING_5M", "CLAUDE_CODE_PROMPT_CACHE_TTL"}}, nil
		case TuneSubCache:
			return tunePlace{path: path, key: "subagentPromptCacheTtl", env: []string{"FORCE_PROMPT_CACHING_5M", "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL"}}, nil
		}
	case "codex":
		if knob == TuneCompact {
			return tunePlace{path: filepath.Join(here(home).codexHome(), "config.toml"), key: "model_auto_compact_token_limit", toml: true, number: true}, nil
		}
	}
	return tunePlace{}, fmt.Errorf("%s has no setting %q tune can put in", agent, knob)
}

func (p tunePlace) get() (string, bool) {
	if p.toml {
		return edit.GetTOMLTop(p.path, p.key)
	}
	return edit.GetJSON(p.path, p.key)
}

// envOf is a variable Claude Code starts with: in settings.json's env, or
// magpie's own, which a terminal it is run from most likely has too.
func (p tunePlace) envOf(k string) string {
	if v, _ := edit.GetJSON(p.path, "env."+k); v != "" {
		return v
	}
	return os.Getenv(k)
}

// TuneRead is how an agent has a setting tune advises now.
func TuneRead(home, agent, knob, model string) (TuneSetting, error) {
	p, err := tuneWhere(home, agent, knob, model)
	if err != nil {
		return TuneSetting{}, err
	}
	var s TuneSetting
	for _, k := range p.env {
		if v := p.envOf(k); v != "" && v != "0" && v != "false" {
			s.Locked = k
			s.Value, s.Source = v, "env"
			if k == "FORCE_PROMPT_CACHING_5M" {
				s.Value = "5m"
			}
			break
		}
	}
	if s.Locked == "" {
		if v, ok := p.get(); ok && v != "auto" {
			s.Value, s.Source = strings.Trim(v, `"'`), "settings"
			if knob == TuneCompact && agent == "claude" {
				s.Source = "model"
			}
		} else if agent == "claude" && knob == TuneCompact {
			// the top-level window serves every model with none of its own
			if v, ok := edit.GetJSON(p.path, "autoCompactWindow"); ok && v != "auto" {
				s.Value, s.Source = v, "settings"
			}
		}
	}
	if st, ok := tuneStashed(agent, knob, model); ok && s.Locked == "" {
		v, _ := p.get()
		s.Ours = strings.Trim(v, `"'`) == st.Set
	}
	return s, nil
}

func tuneStashed(agent, knob, model string) (tuneStash, bool) {
	raw := stashLoad()[tuneKey(agent, knob, model)]
	if raw == "" {
		return tuneStash{}, false
	}
	var st tuneStash
	if json.Unmarshal([]byte(raw), &st) != nil {
		return tuneStash{}, false
	}
	return st, true
}

// TuneApply puts value in as an agent's setting, on the user's say. What
// was there before is kept, the first time only, for TuneUndo. A variable
// that takes precedence is an error: the setting would change nothing.
func TuneApply(home, agent, knob, model, value string) error {
	p, err := tuneWhere(home, agent, knob, model)
	if err != nil {
		return err
	}
	cur, _ := TuneRead(home, agent, knob, model)
	if cur.Locked != "" {
		return fmt.Errorf("%s=%s is set and comes before the setting: take it out first", cur.Locked, cur.Value)
	}
	var v any = value
	if p.number {
		n, err := strconv.Atoi(value)
		if err != nil || n <= 0 {
			return fmt.Errorf("%q is no token count", value)
		}
		if agent == "claude" && (n < 100000 || n > 1000000) {
			return fmt.Errorf("Claude Code takes a window of 100000 to 1000000 tokens, not %d", n)
		}
		v = n
	} else if value != "5m" && value != "1h" {
		return fmt.Errorf("the cache is kept 5m or 1h, not %q", value)
	}
	key := tuneKey(agent, knob, model)
	if _, ok := tuneStashed(agent, knob, model); !ok {
		raw, had := p.get()
		b, _ := json.Marshal(tuneStash{Had: had, Raw: raw, Set: value})
		stash(map[string]string{key: string(b)})
	} else {
		st, _ := tuneStashed(agent, knob, model)
		st.Set = value
		b, _ := json.Marshal(st)
		stash(map[string]string{key: string(b)})
	}
	if p.toml {
		return edit.SetTOMLTop(p.path, edit.KV{Path: p.key, Value: v})
	}
	return edit.SetJSON(p.path, edit.KV{Path: p.key, Value: v})
}

// TuneUndo puts back what a setting was before tune put one in, unless
// the user has changed it since: then it is theirs, and only forgotten.
func TuneUndo(home, agent, knob, model string) error {
	p, err := tuneWhere(home, agent, knob, model)
	if err != nil {
		return err
	}
	st, ok := tuneStashed(agent, knob, model)
	if !ok {
		return fmt.Errorf("magpie didn't set %s", p.key)
	}
	defer forget(tuneKey(agent, knob, model))
	if v, _ := p.get(); strings.Trim(v, `"'`) != st.Set {
		return nil
	}
	if st.Had {
		var v any
		if p.toml {
			v = tomlRaw(st.Raw)
		} else if json.Unmarshal([]byte(st.Raw), &v) != nil {
			v = st.Raw
		}
		if p.toml {
			return edit.SetTOMLTop(p.path, edit.KV{Path: p.key, Value: v})
		}
		return edit.SetJSON(p.path, edit.KV{Path: p.key, Value: v})
	}
	if p.toml {
		return edit.DelTOMLTop(p.path, p.key)
	}
	if err := edit.DelJSON(p.path, p.key); err != nil {
		return err
	}
	// an entry under modelSettings left with nothing in it goes too
	if agent == "claude" && knob == TuneCompact {
		n := claudeName(model)
		empty := func(k string) bool {
			v, ok := edit.GetJSON(p.path, k)
			return ok && strings.Join(strings.Fields(v), "") == "{}"
		}
		if empty("modelSettings." + n) {
			edit.DelJSON(p.path, "modelSettings."+n)
		}
		if empty("modelSettings") {
			edit.DelJSON(p.path, "modelSettings")
		}
	}
	return nil
}

// tomlRaw is a TOML value as GetTOMLTop read it, as a value to write.
func tomlRaw(raw string) any {
	if n, err := strconv.Atoi(raw); err == nil {
		return n
	}
	return strings.Trim(raw, `"'`)
}
