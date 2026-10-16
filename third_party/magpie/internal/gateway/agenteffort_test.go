package gateway

import (
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
)

// Cursor Private Inference can't say an effort in its environment nor, for
// most models, in its app (#1003): the one picked for it in magpie is asked
// on its requests to a model that reasons, whichever API it speaks, in
// place of a level the request sent; another agent's requests, a model
// that doesn't reason and a member a group fixes at a level go as they
// were, and with the pick taken off Cursor's go as it asks.
func TestAgentEffortOnCursorLocal(t *testing.T) {
	s, up := fasted(t)
	if err := provider.Save(provider.Provider{ID: "plain", Name: "Plain", Key: "kp", Chat: "https://plain.example/v1", Models: []string{"gpt-4o-mini"}}); err != nil {
		t.Fatal(err)
	}
	// the levels each vendor lists its model with
	if err := catalog.SaveLive("oa", "https://api.openai.com/v1", []catalog.Model{{ID: "gpt-6.1-sol", Efforts: []string{"low", "medium", "high", "xhigh"}}, {ID: "gpt-6.1-mini"}}); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive("an", "https://api.anthropic.com", []catalog.Model{{ID: "claude-opus-5-5", Reasoning: true}, {ID: "claude-sonnet-5"}}); err != nil {
		t.Fatal(err)
	}
	if p, err := provider.Find("plain"); err != nil || p.Thinks("gpt-4o-mini") {
		t.Fatal("the fixture's plain model reasons")
	}
	if err := provider.SetAgentEffort("cursor-local", "HIGH"); err != nil {
		t.Fatal(err)
	}
	if got := provider.AgentEffort("cursor-local"); got != "high" {
		t.Fatalf("kept %q", got)
	}
	if err := provider.SetAgentEffort("cursor-local", "hard"); err == nil {
		t.Fatal("a word that isn't a level was kept")
	}

	chat := func(model, extra string) string {
		return `{"model":"` + model + `","messages":[{"role":"user","content":"hi"}]` + extra + `}`
	}
	effortOf := func(body map[string]any) any {
		if r, ok := body["reasoning"].(map[string]any); ok {
			return r["effort"]
		}
		return body["reasoning_effort"]
	}
	for _, a := range []struct{ name, path, body string }{
		{"chat, none asked", "/v1/chat/completions", chat("oa/gpt-6.1-sol", "")},
		{"chat, low asked", "/v1/chat/completions", chat("oa/gpt-6.1-sol", `,"reasoning_effort":"low"`)},
		{"responses, none asked", "/v1/responses", `{"model":"oa/gpt-6.1-sol","input":"hi"}`},
		{"responses, low asked", "/v1/responses", `{"model":"oa/gpt-6.1-sol","input":"hi","reasoning":{"effort":"low"}}`},
	} {
		postFrom(t, s, "cursor-local", a.path, a.body)
		if host, _, _, body := up.last(t); host != "api.openai.com" || effortOf(body) != "high" {
			t.Fatalf("%s: sent to %s %v", a.name, host, body)
		}
	}

	// a Claude on Messages thinks, at the pick's level
	postFrom(t, s, "cursor-local", "/v1/messages", `{"model":"an/claude-opus-5-5","max_tokens":32000,"messages":[{"role":"user","content":"hi"}]}`)
	if host, _, _, body := up.last(t); host != "api.anthropic.com" || body["thinking"] == nil {
		t.Fatalf("claude: sent to %s %v", host, body)
	}

	// another agent's request goes as it asked
	postFrom(t, s, "opencode", "/v1/chat/completions", chat("oa/gpt-6.1-sol", `,"reasoning_effort":"low"`))
	if _, _, _, body := up.last(t); effortOf(body) != "low" {
		t.Fatalf("opencode's request took Cursor's pick: %v", body)
	}
	// a model that doesn't reason isn't asked to
	postFrom(t, s, "cursor-local", "/v1/chat/completions", chat("plain/gpt-4o-mini", ""))
	if host, _, _, body := up.last(t); host != "plain.example" || effortOf(body) != nil {
		t.Fatalf("a model that doesn't reason: sent to %s %v", host, body)
	}
	// a group's member fixed at a level keeps it
	if err := provider.SaveGroup(provider.Group{ID: "fx", Name: "FX", Members: []string{"oa/gpt-6.1-sol:low"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	postFrom(t, s, "cursor-local", "/v1/chat/completions", chat("group/fx", ""))
	if _, _, _, body := up.last(t); effortOf(body) != "low" {
		t.Fatalf("a member fixed at low: %v", body)
	}

	// taken off: Cursor's requests go as it asks
	if err := provider.SetAgentEffort("cursor-local", ""); err != nil {
		t.Fatal(err)
	}
	postFrom(t, s, "cursor-local", "/v1/chat/completions", chat("oa/gpt-6.1-sol", `,"reasoning_effort":"low"`))
	if _, _, _, body := up.last(t); effortOf(body) != "low" {
		t.Fatalf("taken off, still asked: %v", body)
	}
	postFrom(t, s, "cursor-local", "/v1/chat/completions", chat("oa/gpt-6.1-sol", ""))
	if _, _, _, body := up.last(t); effortOf(body) != nil {
		t.Fatalf("taken off, an effort was added: %v", body)
	}
}
