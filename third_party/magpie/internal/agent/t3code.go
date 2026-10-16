package agent

// T3 Code (pingdotgg/t3code, a GUI that drives Claude Code, Codex, Cursor
// and others; KevinXC on Discord) keeps its settings in
// ~/.t3/userdata/settings.json ($T3CODE_HOME/userdata), a sparse JSON file
// its server watches and reloads on an outside edit. Each provider it lists
// is an instance of a driver, and besides the built-in ones (keyed by the
// driver, "claudeAgent", "codex", …) it takes instances of one's own:
//
//	{"providerInstances":{"<id>":{"driver":"claudeAgent","displayName":…,
//	  "enabled":true,"environment":[{"name":…,"value":…,"sensitive":false}],
//	  "config":{"customModels":[{"slug":…,"name":…,"capabilities":
//	    {"optionDescriptors":[{"id":"effort","type":"select",…}]}}]}}}}
//
// A claudeAgent instance runs Claude Code (Anthropic's Agent SDK, the
// user's ~/.claude settings read as Claude Code reads them) with the
// instance's environment added, and lists Claude's models and its custom
// models, each sent as Claude Code's model. So magpie is an instance of its
// own, "magpie": Claude Code pointed at the gateway (ANTHROPIC_BASE_URL and
// the token Claude Code is routed with, so its requests are Claude Code's
// to the gateway, tiers and all), every magpie model one of its custom
// models, a 1M one marked [1m] as magpie marks it for Claude Code, with
// a Reasoning pick of the levels it takes and a Thinking switch
// (t3Capabilities). It is
// magpie's alone: the user's own instances, their custom models and T3's
// other keys stay as they are, and off takes only it out. A binary path or
// Claude home the user gave T3's own Claude is carried over, so it runs the
// same Claude Code. T3's Codex lists what Codex's model/list says, which is
// magpie's catalog once Codex is routed through magpie, so it needs nothing.
//
// The settings environment (~/.claude/settings.json's env) is applied over
// the process', so a Claude Code routed elsewhere in its own settings
// takes its requests there; one routed through magpie or not routed at all
// asks the gateway. A provider switch on there (CLAUDE_CODE_USE_BEDROCK, …)
// takes it to that cloud, which Check says.

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"

	"github.com/yetone/magpie/internal/agentenv"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/provider"
)

// t3Instance is the key of magpie's provider instance in T3 Code's
// settings.json.
const t3Instance = "providerInstances." + magpieID

func t3code(home string) *Agent {
	base := appdir.Getenv("T3CODE_HOME")
	if base == "" {
		base = filepath.Join(home, ".t3")
	} else if rest, ok := strings.CutPrefix(base, "~"); ok {
		base = filepath.Join(home, rest)
	}
	path := filepath.Join(base, "userdata", "settings.json")
	wired := func() bool { _, ok := edit.GetJSON(path, t3Instance); return ok }
	return &Agent{
		ID: "t3code", Name: "T3 Code", Icon: "t3code", Aliases: []string{"t3", "t3-code"},
		Dir: base, Path: path,
		// ~/.t3 is made at its first start; a Mac app never opened yet is
		// found by its bundle. No command: `t3` is a common name.
		detect: func() bool { return isDir(base) || t3App(home) },
		Sync: func() error {
			return syncJSON(path, t3Instance, func() any { return t3Merged(path, gateway.URL()) })
		},
		// its custom models are the models picked for it (#1508)
		ListsModels: true,
		// magpie's instance still on the gateway, as for every agent with
		// a Check (TestDriftUnwiredEveryAgent, TestPortMovesEveryAgent).
		// The user's ~/.claude settings env is put over the instance's, so
		// a provider switch on there (or in the shell T3 was started from)
		// takes magpie's instance to that cloud too; magpie only says so,
		// as that file is Claude Code's, wired or not
		Check: func() string {
			if !wired() {
				return ""
			}
			if why := wiringOff("T3 Code", path, func(k string) (string, bool) { return t3EnvOf(path, k) }, "ANTHROPIC_BASE_URL", gateway.URL()); why != "" {
				return why
			}
			at := here(home)
			return claudeCloud(at, filepath.Join(at.claudeDir(), "settings.json"))
		},
		Fields: []Field{{
			Key: "provider", Label: "provider",
			Get: func() string {
				if wired() {
					return magpieID
				}
				return ""
			},
			Set: func(v string) error {
				if v == "" {
					return edit.DelJSON(path, t3Instance)
				}
				return edit.SetJSON(path, edit.KV{Path: t3Instance, Value: t3Merged(path, gateway.URL())})
			},
			Options: func(map[string]string) []Option {
				return []Option{{Value: magpieID, Label: "magpie", Icon: "magpie", Note: "every magpie model as a provider in T3 Code, on Claude Code"}}
			},
		}},
	}
}

// t3App reports whether T3 Code's app is in a Mac's Applications, by the
// names its builds take.
func t3App(home string) bool {
	if runtime.GOOS != "darwin" {
		return false
	}
	for _, d := range macAppFolders(home) {
		for _, n := range []string{"T3 Code (Alpha).app", "T3 Code.app"} {
			if _, err := os.Stat(filepath.Join(d, n)); err == nil {
				return true
			}
		}
	}
	return false
}

// t3InstanceJSON is magpie's provider instance in T3 Code's settings.json
// at path: a claudeAgent instance named magpie on the gateway at gw, with
// every magpie model as a custom model, and the binary path and Claude
// home of T3's own Claude when the user gave them one.
func t3InstanceJSON(path, gw string) map[string]any {
	mark := claude1MFor("t3code")
	models := []map[string]any{}
	for _, m := range magpieModels("t3code") {
		// Claude Code reads a dotted Claude version as Claude Opus 4
		e := map[string]any{"slug": provider.ClaudeSpelled(mark(m.ID)), "name": m.Name}
		if c := t3Capabilities(m.ID, m.Efforts); c != nil {
			e["capabilities"] = c
		}
		models = append(models, e)
	}
	config := map[string]any{"customModels": models}
	for _, k := range []string{"binaryPath", "homePath"} {
		// the user's Claude instance, else the setting it was before T3
		// had instances
		v, _ := edit.GetJSON(path, "providerInstances.claudeAgent.config."+k)
		if v == "" {
			v, _ = edit.GetJSON(path, "providers.claudeAgent."+k)
		}
		if v = strings.TrimSpace(v); v != "" {
			config[k] = v
		}
	}
	environment := []map[string]any{
		{"name": "ANTHROPIC_BASE_URL", "value": gw, "sensitive": false},
		{"name": "ANTHROPIC_AUTH_TOKEN", "value": keyAt(gw), "sensitive": false},
	}
	// auto mode's review off while no model it lists is one Anthropic
	// reviews, as for Claude Code itself (claudeReviewEnv); a value in the
	// user's ~/.claude settings comes after it
	if !claudeReviewed([]string{"t3code"}) {
		environment = append(environment, map[string]any{"name": claudeReviewEnv, "value": "0", "sensitive": false})
	}
	return map[string]any{
		"driver": "claudeAgent", "displayName": "magpie", "enabled": true,
		"environment": environment,
		"config":      config,
	}
}

// t3Env is the environment variables of magpie's instance that magpie
// sets: the gateway's address and key, and auto mode's review.
var t3Env = []string{"ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", claudeReviewEnv}

// t3EnvOf is the value of the environment variable named k on magpie's
// instance in the T3 settings at path.
func t3EnvOf(path, k string) (string, bool) {
	cur, ok := edit.GetJSON(path, t3Instance+".environment")
	if !ok {
		return "", false
	}
	var env []map[string]any
	if json.Unmarshal([]byte(cur), &env) != nil {
		return "", false
	}
	for _, v := range env {
		if n, _ := v["name"].(string); n == k {
			s, _ := v["value"].(string)
			return s, true
		}
	}
	return "", false
}

// t3Merged is magpie's instance as t3InstanceJSON writes it, merged into
// the one at path: what the user set on it in T3 Code's own settings
// stays (#2030, Kanscape: its name and Auto-compact after were reset on
// every start). Its name, whether it is on, and every key magpie doesn't
// write (config.autoCompactWindow, config.launchArgs, accentColor) are
// theirs, as are the environment variables besides magpie's own. The
// custom models and the gateway's variables are magpie's, and so is a
// binary path or Claude home while T3's own Claude has one to carry over.
// Only magpie's instance is read: another, such as T3's own Claude, lends
// it nothing of the user's.
func t3Merged(path, gw string) any {
	want := t3InstanceJSON(path, gw)
	cur, ok := edit.GetJSON(path, t3Instance)
	if !ok {
		return want
	}
	var have map[string]json.RawMessage
	if json.Unmarshal([]byte(cur), &have) != nil || have == nil {
		return want
	}
	var name string
	if json.Unmarshal(have["displayName"], &name) == nil && strings.TrimSpace(name) != "" {
		want["displayName"] = name
	}
	var on bool
	if json.Unmarshal(have["enabled"], &on) == nil {
		want["enabled"] = on
	}
	var theirs []map[string]any
	if json.Unmarshal(have["environment"], &theirs) == nil {
		env := want["environment"].([]map[string]any)
		for _, v := range theirs {
			// a provider switch would take magpie's instance past the
			// gateway (claudeClouds), so it doesn't stay on magpie's
			if n, _ := v["name"].(string); n != "" && !slices.Contains(t3Env, n) && !slices.Contains(agentenv.ClaudeProviders, n) {
				env = append(env, v)
			}
		}
		want["environment"] = env
	}
	b, err := json.Marshal(want)
	var w map[string]json.RawMessage
	if err != nil || json.Unmarshal(b, &w) != nil {
		return want
	}
	return mergeTheirs(have, w, []string{"environment"})
}

// t3EffortLabels are T3 Code's names for Claude Code's levels, as its own
// Claude models show them.
var t3EffortLabels = map[string]string{"low": "Low", "medium": "Medium", "high": "High", "xhigh": "Extra High", "max": "Max"}

// t3Capabilities is the picks T3 Code shows beside a custom model: without
// them it shows none and sends no effort, so every magpie model ran at
// whatever Claude Code took (KevinXC on Discord). Reasoning is the levels
// of Claude Code's the model takes, medium chosen first as on T3's own
// Claude models (else the lowest); T3 hands the level to Claude Code as it
// is, which sends it to the gateway. Thinking, on at first, is the switch
// T3's own Claude Haiku has, for a model that thinks at some level: T3 sets
// Claude Code's alwaysThinkingEnabled by it, and Claude Code's requests with
// thinking off ask the gateway with none. nil for a model with neither.
func t3Capabilities(id string, efforts []string) map[string]any {
	var descriptors []map[string]any
	var options []map[string]any
	for _, l := range claudeEffortsFor(id) {
		if contains(efforts, l) {
			options = append(options, map[string]any{"id": l, "label": t3EffortLabels[l]})
		}
	}
	if len(options) > 0 {
		def := options[0]
		for _, o := range options {
			if o["id"] == "medium" {
				def = o
			}
		}
		def["isDefault"] = true
		descriptors = append(descriptors, map[string]any{"id": "effort", "label": "Reasoning", "type": "select", "options": options})
	}
	if slices.ContainsFunc(efforts, func(l string) bool { return l != "none" }) {
		descriptors = append(descriptors, map[string]any{"id": "thinking", "label": "Thinking", "type": "boolean", "currentValue": true})
	}
	if len(descriptors) == 0 {
		return nil
	}
	return map[string]any{"optionDescriptors": descriptors}
}
