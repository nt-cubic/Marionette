package gui

import (
	"encoding/base64"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// Codex's own models, a ChatGPT account's, stay in its model picker with a
// routing group switched on and its own models switched off (shiqi_40904 on
// Discord: "开启路由组后 … 模型列表里不会显示 codex 模型"). magpie's list shows
// them Codex as its Codex section, and its pickers offer them as Codex's own
// (codex/… would only add a hop), so the switch for the agent's own models
// never takes them out. It did once the group gave the picker one of
// magpie's models, which is what the switch waits for: the picker was the
// group alone, and with the group off every model was back.
func TestCodexOwnModelsStayWithGroupOn(t *testing.T) {
	ids := codexSignedInSandbox(t)
	if err := provider.SetOwnModelsHidden("codex", true); err != nil {
		t.Fatal(err)
	}
	codex, err := agent.Find("codex")
	if err != nil {
		t.Fatal(err)
	}
	picks := func() (out []string) {
		t.Helper()
		for _, f := range agentFields(codex, codex.Values()) {
			if f.Key == "model" {
				for _, o := range f.Options {
					out = append(out, o.Value)
				}
			}
		}
		return out
	}
	for _, id := range ids {
		if got := picks(); !slices.Contains(got, id) {
			t.Fatalf("group off: %s not in Codex's picker %v", id, got)
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "mygpt", Name: "MyGPT", Members: []string{"codex/gpt-5.5", "codex/gpt-5.4"}}); err != nil {
		t.Fatal(err)
	}
	got := picks()
	for _, id := range append(slices.Clone(ids), "group/mygpt") {
		if !slices.Contains(got, id) {
			t.Fatalf("group on: %s not in Codex's picker %v", id, got)
		}
	}
	// nothing of Codex's own is left for the switch to take out, so the
	// list has none to show
	if codex.OwnModels(codex.Values()) {
		t.Fatal("Codex's list offers a switch for its own models, every one of them magpie's too")
	}
	// one the list takes out is the switch's again: magpie no longer shows
	// it Codex
	if err := provider.SetHiddenModels("codex", []string{"codex/gpt-5.4"}); err != nil {
		t.Fatal(err)
	}
	if got := picks(); slices.Contains(got, "gpt-5.4") || !slices.Contains(got, "gpt-5.4-mini") {
		t.Fatalf("gpt-5.4 taken out of the list, own models off: picker %v", got)
	}
}

// codexSignedInSandbox signs a ChatGPT account in to a sandbox Codex, with
// the models its backend lists, and returns their ids.
func codexSignedInSandbox(t *testing.T) []string {
	t.Helper()
	home := sandboxHome(t)
	codexHome := filepath.Join(home, ".codex")
	t.Setenv("CODEX_HOME", codexHome)
	os.MkdirAll(codexHome, 0o700)
	jwt := func(c any) string {
		b, _ := json.Marshal(c)
		return "e30." + base64.RawURLEncoding.EncodeToString(b) + ".sig"
	}
	auth, _ := json.Marshal(map[string]any{"auth_mode": "chatgpt", "tokens": map[string]any{
		"id_token":     jwt(map[string]any{"email": "a@x.y", "https://api.openai.com/auth": map[string]any{"chatgpt_plan_type": "plus", "chatgpt_account_id": "acc"}}),
		"access_token": jwt(map[string]any{"exp": time.Now().Add(24 * time.Hour).Unix()}), "refresh_token": "r", "account_id": "acc"}})
	os.WriteFile(filepath.Join(codexHome, "auth.json"), auth, 0o600)
	os.WriteFile(filepath.Join(codexHome, "config.toml"), []byte("model = \"gpt-5.5\"\n"), 0o600)
	ids := []string{"gpt-5.5", "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex", "gpt-5.2"}
	var cache []map[string]any
	var live []catalog.Model
	for i, id := range ids {
		cache = append(cache, map[string]any{"slug": id, "display_name": id, "visibility": "list", "priority": i, "context_window": 400000})
		live = append(live, catalog.Model{ID: id, Context: 400000})
	}
	b, _ := json.Marshal(map[string]any{"etag": "x", "models": cache})
	os.WriteFile(filepath.Join(codexHome, "models_cache.json"), b, 0o600)
	provider.ForgetAccounts()
	t.Cleanup(provider.ForgetAccounts)
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	if err := catalog.SaveLive("codex", provider.CodexBase, live); err != nil {
		t.Fatal(err)
	}
	return ids
}

// The codex provider "Only through routing groups" (只通过路由分组使用) keeps
// the ChatGPT account's models out of other agents' lists but in Codex's
// own, and a group that lists itself alone keeps them in every agent's
// (士奇 on Discord, 10-28: "开启路由组以后，agent 设置页面，模型列表里就是丢失 codex 供应商和对应模型，在下拉
// 选项中虽然能显示 codex 模型但是实际codex 应用中是没有的"). Codex's list
// has its Codex section, so its picker offers them as magpie's and there is
// no own-models switch to show them by.
func TestCodexOwnModelsListedOnlyThroughGroups(t *testing.T) {
	ids := codexSignedInSandbox(t)
	codex, err := agent.Find("codex")
	if err != nil {
		t.Fatal(err)
	}
	check := func(when string, unlisted ...string) {
		t.Helper()
		own := map[string]bool{}
		for _, m := range agentModelList(codex) {
			if m.Own {
				own[m.ID] = true
			}
		}
		for _, id := range ids {
			if !own["codex/"+id] {
				t.Errorf("%s: codex/%s not in Codex's list %v", when, id, own)
			}
		}
		if codex.OwnModels(codex.Values()) {
			t.Errorf("%s: Codex's list offers a switch for its own models", when)
		}
		var picks []string
		for _, f := range agentFields(codex, codex.Values()) {
			if f.Key == "model" {
				for _, o := range f.Options {
					picks = append(picks, o.Value)
				}
			}
		}
		for _, id := range ids {
			if !slices.Contains(picks, id) {
				t.Errorf("%s: %s not in Codex's picker %v", when, id, picks)
			}
		}
		for _, e := range func() []provider.Entry { l, _ := provider.ListedFor("claude"); return l }() {
			if provider.CodexOwn(e) && slices.Contains(unlisted, e.ID) {
				t.Errorf("%s: Claude Code lists %s", when, e.ID)
			}
		}
	}
	if err := provider.SaveGroup(provider.Group{ID: "mygpt", Name: "MyGPT", Members: []string{"codex/gpt-5.5", "codex/gpt-5.4"}, Unlists: true}); err != nil {
		t.Fatal(err)
	}
	// a group listing alone no longer keeps a subscription's models from
	// any agent (TestSubscriptionModelsStayInEveryAgentsListWithGroupOn):
	// Claude Code lists them too
	check("group lists itself alone")
	p, err := provider.Find("codex")
	if err != nil {
		t.Fatal(err)
	}
	q := *p
	q.Unlisted = true
	if err := provider.Save(q); err != nil {
		t.Fatal(err)
	}
	var all []string
	for _, id := range ids {
		all = append(all, "codex/"+id)
	}
	check("only through routing groups", all...)
}

// A group that lists itself alone (Group.Unlists) keeps its models out
// of a client's /v1/models (#1783), but a signed-in subscription's models
// stay in every agent's list with it switched on: the group is added on
// top (士奇 on Discord, a third time: with the group on, Hermes Agent's and
// WorkBuddy's lists lost the Codex section and every GPT model, and off
// they were back; a3ee00c05 kept them in Codex's list alone). Every agent
// is checked, its Agents-page list and its model pickers, so a fix for one
// agent can't pass for all.
func TestSubscriptionModelsStayInEveryAgentsListWithGroupOn(t *testing.T) {
	ids := codexSignedInSandbox(t)
	var refs []string
	for _, id := range ids {
		refs = append(refs, "codex/"+id)
	}
	listed := func(a *agent.Agent) map[string]bool {
		out := map[string]bool{}
		for _, m := range agentModelList(a) {
			out[m.ID] = true
		}
		return out
	}
	// the group names every one of them, as the reporter's took all five
	if err := provider.SaveGroup(provider.Group{ID: "mygpt", Name: "MyGPT", Members: refs, Unlists: true}); err != nil {
		t.Fatal(err)
	}
	checked := 0
	for _, a := range agent.All() {
		got := listed(a)
		if !got["group/mygpt"] {
			continue // an agent shown no models of magpie's (or no groups)
		}
		checked++
		for _, ref := range refs {
			if !got[ref] {
				t.Errorf("group on: %s not in %s's list %v", ref, a.ID, got)
			}
		}
		if shown, _ := provider.CatalogFor(a.ListsFor()); !slices.ContainsFunc(shown, func(e provider.Entry) bool { return e.ID == refs[0] }) {
			t.Errorf("group on: %s not in what %s is written and /v1/models gives it", refs[0], a.ID)
		}
		var picks []string
		for _, f := range agentFields(a, a.Values()) {
			if f.Key != "model" {
				continue
			}
			for _, o := range f.Options {
				picks = append(picks, o.Value)
			}
		}
		if slices.ContainsFunc(picks, func(v string) bool { return strings.HasSuffix(v, "group/mygpt") }) &&
			!slices.ContainsFunc(picks, func(v string) bool { return v == ids[0] || strings.HasSuffix(v, refs[0]) }) {
			t.Errorf("group on: %s's picker offers the group but not %s: %v", a.ID, refs[0], picks)
		}
	}
	for _, id := range []string{"hermes", "workbuddy", "claude", "codex"} {
		a, err := agent.Find(id)
		if err != nil {
			t.Fatal(err)
		}
		if !listed(a)["group/mygpt"] {
			t.Errorf("%s's list wasn't checked: it has no group/mygpt", id)
		}
	}
	if checked < 10 {
		t.Errorf("only %d agents' lists checked", checked)
	}
	// a client that is no agent is still given the group alone (#1783)
	for _, e := range provider.Catalog() {
		if slices.Contains(refs, e.ID) {
			t.Errorf("the gateway's /v1/models for a client lists %s, kept for the group", e.ID)
		}
	}
	if shown, _ := provider.CatalogFor(""); slices.ContainsFunc(shown, func(e provider.Entry) bool { return slices.Contains(refs, e.ID) }) {
		t.Error("a client that is no agent is listed the group's models")
	}
	// the picker's "kept for routing groups" note doesn't name a model the
	// agents are offered
	for _, m := range unlistedModels() {
		if slices.Contains(refs, m.ID) {
			t.Errorf("%s said kept for routing groups, while every agent lists it", m.ID)
		}
	}
	// the codex provider kept for routing groups itself still keeps them
	// out of the other agents' lists: the user's own switch, which a group
	// switched off doesn't bring back
	p, err := provider.Find("codex")
	if err != nil {
		t.Fatal(err)
	}
	q := *p
	q.Unlisted = true
	if err := provider.Save(q); err != nil {
		t.Fatal(err)
	}
	hermes, _ := agent.Find("hermes")
	if got := listed(hermes); got[refs[0]] {
		t.Errorf("codex used only through routing groups: Hermes lists %s", refs[0])
	}
}
