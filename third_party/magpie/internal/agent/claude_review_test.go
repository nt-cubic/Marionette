package agent

import (
	"encoding/json"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
)

// TestClaudeAutoModeReview: Claude Code in auto mode behind magpie on a
// model Anthropic doesn't review (1EFLOPS on X, on Codex's gpt-6.1-sol)
// said "this session isn't eligible because your requests go through
// 127.0.0.1:3425…". Anthropic's page says to turn the review off
// (CLAUDE_CODE_AUTO_MODE_SERVER=0) for a gateway that can't run it: magpie
// does while no model Claude Code can reach through it is reviewed, and
// leaves it on while one is, shown or behind a group or a fallback. A
// value of the user's is theirs, and magpie's goes when Claude Code is
// taken off magpie.
func TestClaudeAutoModeReview(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("APPDATA", filepath.Join(home, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(home, "AppData", "Local"))
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}}); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(home, ".claude", "settings.json")
	writeFile(t, path, `{"theme":"dark"}`)
	a := claude(home)
	review := func() string { v, _ := edit.GetJSON(path, "env."+claudeReviewEnv); return v }
	set := func(key, v string) {
		t.Helper()
		if err := a.Field(key).Set(v); err != nil {
			t.Fatal(err)
		}
	}
	sync := func() {
		t.Helper()
		if err := a.Sync(); err != nil {
			t.Fatal(err)
		}
	}

	// only a model no one reviews: off
	set("model", "v/gpt-x")
	if review() != "0" {
		t.Fatalf("on v/gpt-x alone: %q, want 0", review())
	}
	sync()
	if review() != "0" {
		t.Fatalf("Sync: %q", review())
	}

	// a Claude model on an Anthropic endpoint joins: on, as /model can take
	// the session there
	if err := provider.Save(provider.Provider{ID: "a", Name: "A", Anthropic: "http://127.0.0.1:1", Key: "k", Models: []string{"claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "" {
		t.Fatalf("with a/claude-sonnet-5 shown: %q, want unset", review())
	}

	// hidden from Claude Code: off again; reachable through a group shown
	// to it, or a provider's fallback, on
	if err := provider.SetHiddenModels("claude", []string{"a/claude-sonnet-5"}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "0" {
		t.Fatalf("Claude hidden: %q, want 0", review())
	}
	if err := provider.SaveGroup(provider.Group{Name: "mix", Members: []string{"v/gpt-x", "a/claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "" {
		t.Fatalf("a group with a Claude member: %q, want unset", review())
	}
	// a group that keeps its models out of the lists (#1783) still routes
	// to them, so its Claude member counts
	if err := provider.SetHiddenModels("claude", nil); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "mix", Members: []string{"v/gpt-x", "a/claude-sonnet-5"}, Unlists: true}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "" {
		t.Fatalf("a group unlisting its Claude member: %q, want unset", review())
	}
	if err := provider.SetHiddenModels("claude", []string{"a/claude-sonnet-5"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.DeleteGroup("mix"); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "0" {
		t.Fatalf("group deleted: %q, want 0", review())
	}
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}, Fallback: []string{"a/claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "" {
		t.Fatalf("falling back to Claude: %q, want unset", review())
	}
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}}); err != nil {
		t.Fatal(err)
	}
	sync()
	if review() != "0" {
		t.Fatalf("fallback gone: %q, want 0", review())
	}

	// off magpie, onto Claude Code's own model, and reset: magpie's goes
	set("model", "claude-opus-4-8")
	if review() != "" {
		t.Fatalf("Claude's own model kept magpie's value: %q", review())
	}
	set("model", "v/gpt-x")
	if review() != "0" {
		t.Fatalf("back on v/gpt-x: %q", review())
	}
	set("model", "")
	if review() != "" {
		t.Fatalf("reset kept magpie's value: %q", review())
	}

	// the user's own value is theirs, through a switch, Sync and reset
	for _, mine := range []string{"1", "0"} {
		if err := edit.SetJSON(path, edit.KV{Path: "env." + claudeReviewEnv, Value: mine}); err != nil {
			t.Fatal(err)
		}
		set("model", "v/gpt-x")
		sync()
		if err := provider.Save(provider.Provider{ID: "a", Name: "A", Anthropic: "http://127.0.0.1:1", Key: "k", Models: []string{"claude-sonnet-5"}}); err != nil {
			t.Fatal(err)
		}
		if err := provider.SetHiddenModels("claude", nil); err != nil {
			t.Fatal(err)
		}
		sync()
		if review() != mine {
			t.Fatalf("the user's %q became %q", mine, review())
		}
		set("model", "")
		if review() != mine {
			t.Fatalf("reset took the user's %q away: %q", mine, review())
		}
		if err := edit.DelJSON(path, "env."+claudeReviewEnv); err != nil {
			t.Fatal(err)
		}
		if err := provider.SetHiddenModels("claude", []string{"a/claude-sonnet-5"}); err != nil {
			t.Fatal(err)
		}
	}
	if v, _ := edit.GetJSON(path, "theme"); v != "dark" {
		t.Fatal("theme lost")
	}
}

// TestT3CodeAutoModeReview: T3 Code's magpie instance runs Claude Code on
// the gateway through its own environment, so it is told the same: the
// review off while none of the models it lists is reviewed, and on (not
// set) once one is.
func TestT3CodeAutoModeReview(t *testing.T) {
	home := syncHome(t)
	t.Setenv("T3CODE_HOME", "")
	path := filepath.Join(home, ".t3", "userdata", "settings.json")
	writeFile(t, path, `{}`)
	a := t3code(home)
	if err := a.Field("provider").Set(magpieID); err != nil {
		t.Fatal(err)
	}
	review := func() (string, bool) {
		t.Helper()
		var file map[string]map[string]json.RawMessage
		if err := json.Unmarshal([]byte(readFile(path)), &file); err != nil {
			t.Fatal(err)
		}
		for _, v := range t3Format(t, file["providerInstances"][magpieID]).Environment {
			if v.Name == claudeReviewEnv {
				return v.Value, true
			}
		}
		return "", false
	}
	if v, ok := review(); v != "0" || !ok {
		t.Fatalf("relay/glm-4.6 alone: %q %v, want 0", v, ok)
	}
	if err := provider.Save(provider.Provider{ID: "a", Name: "A", Anthropic: "http://127.0.0.1:1", Key: "k", Models: []string{"claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if v, ok := review(); ok {
		t.Fatalf("with a/claude-sonnet-5 listed: %q, want none", v)
	}
}

// TestClaudeReviewedJudgesTheSetModelWhereItGoes: a model Claude Code is
// set to is judged on the provider it resolves to, as the gateway judges
// its requests, not by its name alone. A Claude name on a provider that
// falls back to Anthropic, or a group with a Claude member, keeps the
// review on; a name magpie doesn't know is judged by the name.
func TestClaudeReviewedJudgesTheSetModelWhereItGoes(t *testing.T) {
	syncHome(t)
	if err := provider.Save(provider.Provider{ID: "a", Name: "A", Anthropic: "http://127.0.0.1:1", Key: "k", Models: []string{"claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}}); err != nil {
		t.Fatal(err)
	}
	if claudeReviewed(nil, "v/gpt-x") {
		t.Fatal("v/gpt-x alone counted as reviewed")
	}
	if !claudeReviewed(nil, "a/claude-sonnet-5") || !claudeReviewed(nil, "claude-opus-4-8") {
		t.Fatal("a Claude model not reviewed")
	}
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}, Fallback: []string{"a/claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	if !claudeReviewed(nil, "v/gpt-x") {
		t.Fatal("v/gpt-x falling back to a/claude-sonnet-5 not reviewed")
	}
	if err := provider.Save(provider.Provider{ID: "v", Name: "V", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"gpt-x"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "mix", Members: []string{"v/gpt-x", "a/claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	if _, _, ok := provider.FindGroup(provider.GroupPrefix + "mix"); !ok {
		t.Fatal("group mix not found")
	}
	if !claudeReviewed(nil, provider.GroupPrefix+"mix") {
		t.Fatal("group mix with a Claude member not reviewed")
	}
}

// TestClaudeReviewedOnlyWhereMessagesReachClaude (nexnull on Discord,
// Claude Code 2.1.308 on DeepSeek through magpie v0.1.1318, still told
// "this session isn't eligible because your requests go through
// 127.0.0.1:3425"): a Claude model a provider serves only on Chat or
// Responses is asked translated, and a translation carries neither the
// safeguards nor their results, so it can't keep the review on. A relay
// with no Anthropic endpoint listing claude-sonnet-5 beside DeepSeek, or a
// Claude model the user set to be asked on chat, leaves the review off; the
// same model asked on the relay's Anthropic endpoint turns it back on.
func TestClaudeReviewedOnlyWhereMessagesReachClaude(t *testing.T) {
	home := syncHome(t)
	path := filepath.Join(home, ".claude", "settings.json")
	writeFile(t, path, `{}`)
	review := func() string { v, _ := edit.GetJSON(path, "env."+claudeReviewEnv); return v }
	a := claude(home)
	relay := provider.Provider{ID: "r", Name: "R", Chat: "http://127.0.0.1:1/v1", Key: "k", Models: []string{"deepseek-v4.1-flash", "claude-sonnet-5"}}
	if err := provider.Save(relay); err != nil {
		t.Fatal(err)
	}
	if err := a.Field("model").Set("r/deepseek-v4.1-flash"); err != nil {
		t.Fatal(err)
	}
	if claudeReviewed(nil, "r/claude-sonnet-5") {
		t.Error("a Claude model asked on chat alone counted as reviewed")
	}
	if review() != "0" {
		t.Fatalf("a relay without an Anthropic endpoint: %q, want 0", review())
	}

	// the relay's Anthropic endpoint reaches Claude: on
	relay.Anthropic = "http://127.0.0.1:1"
	if err := provider.Save(relay); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if review() != "" {
		t.Fatalf("Claude on the relay's Anthropic endpoint: %q, want unset", review())
	}

	// set to be asked on chat there: off again
	if err := provider.SetModelAPI("r/claude-sonnet-5", "chat"); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if review() != "0" {
		t.Fatalf("Claude set to be asked on chat: %q, want 0", review())
	}
	if err := provider.SetModelAPI("r/claude-sonnet-5", ""); err != nil {
		t.Fatal(err)
	}
	if err := a.Sync(); err != nil {
		t.Fatal(err)
	}
	if review() != "" {
		t.Fatalf("back to the vendor's list: %q, want unset", review())
	}
}
