package provider

import (
	"testing"

	"github.com/yetone/magpie/internal/plugin"
)

// A Claude model through the Copilot plugin goes to Copilot's
// /v1/messages: the loader's base is the bare https://api.githubcopilot.com
// for every model, and the model's own URL (models.dev's) says /v1. Sent to
// the bare base it went to /messages and Copilot answered 404 (cyoking1 on
// X). What already ends in /v1, what isn't Anthropic's, and a model whose
// own URL says no /v1 go as they did.
func TestPluginAnthropicBaseKeepsV1(t *testing.T) {
	copilot := plugin.Provider{ID: "github-copilot", NPM: "@ai-sdk/openai-compatible", API: "https://api.githubcopilot.com"}
	claude := plugin.Model{ID: "claude-sonnet-5", NPM: "@ai-sdk/anthropic", URL: "https://api.githubcopilot.com/v1"}
	gpt := plugin.Model{ID: "gpt-5.5", NPM: "@ai-sdk/github-copilot", URL: "https://api.githubcopilot.com"}
	kiro := plugin.Provider{ID: "kiro", NPM: "@ai-sdk/anthropic"}
	kiroModel := plugin.Model{ID: "claude-sonnet-4.5", NPM: "@ai-sdk/anthropic", URL: "https://runtime.us-east-1.kiro.dev"}
	for _, c := range []struct {
		name   string
		pp     plugin.Provider
		m      plugin.Model
		loader string
		want   string
	}{
		{"copilot claude", copilot, claude, "https://api.githubcopilot.com", "https://api.githubcopilot.com/v1"},
		{"copilot claude, slash", copilot, claude, "https://api.githubcopilot.com/", "https://api.githubcopilot.com/v1"},
		{"copilot enterprise claude", copilot, claude, "https://copilot-api.example.ghe.com", "https://copilot-api.example.ghe.com/v1"},
		{"copilot gpt", copilot, gpt, "https://api.githubcopilot.com", "https://api.githubcopilot.com"},
		{"loader base has /v1", copilot, claude, "https://api.githubcopilot.com/v1", "https://api.githubcopilot.com/v1"},
		{"no loader base", copilot, claude, "", "https://api.githubcopilot.com/v1"},
		{"model url without /v1", kiro, kiroModel, "https://runtime.us-east-1.kiro.dev", "https://runtime.us-east-1.kiro.dev"},
		{"minimax", plugin.Provider{ID: "minimax"}, plugin.Model{ID: "MiniMax-M3", NPM: "@ai-sdk/anthropic", URL: "https://x.example/mavis/api/v1/llm/v1"}, "https://x.example/mavis/api/v1/llm/v1", "https://x.example/mavis/api/v1/llm/v1"},
	} {
		if got := pluginRequestBase(c.pp, c.m, c.loader); got != c.want {
			t.Errorf("%s: base %q, want %q", c.name, got, c.want)
		}
	}
}
