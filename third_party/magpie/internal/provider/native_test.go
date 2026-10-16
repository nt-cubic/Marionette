package provider

import (
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// Native is the API a client does best to ask a model on, for the gateway
// to relay it as it is.
func TestNative(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	catalog.Reset()
	t.Cleanup(catalog.Reset)
	// Copilot's model list says which APIs serve each model
	if err := catalog.SaveLive("copilot", "https://api.githubcopilot.com", []catalog.Model{
		{ID: "gpt-5.5", APIs: []string{"responses"}}, {ID: "claude-sonnet-5", APIs: []string{"chat", "anthropic"}}}); err != nil {
		t.Fatal(err)
	}
	openai := Provider{ID: "openai", Chat: "https://api.openai.com/v1", Responses: "https://api.openai.com/v1"}
	copilot := Provider{ID: "copilot", Chat: "https://api.githubcopilot.com", Responses: "https://api.githubcopilot.com", Anthropic: "https://api.githubcopilot.com"}
	for _, c := range []struct {
		name  string
		p     Provider
		model string
		want  Protocol
	}{
		{"chat only", Provider{ID: "relay", Chat: "https://relay.test/v1"}, "glm-4.6", Chat},
		{"responses only", Provider{ID: "resp", Responses: "https://resp.test/v1"}, "m", Responses},
		{"ChatGPT sign-in", Provider{ID: "codex", Responses: CodexBase, Account: &Account{Agent: "codex"}}, "gpt-5.5", Responses},
		{"GPT on OpenAI's API", openai, "gpt-5.5", Responses},
		{"o-series on OpenAI's API", openai, "o3", Responses},
		{"other on OpenAI's API", openai, "whisper-x", Chat},
		{"Chat and Responses elsewhere", Provider{ID: "both", Chat: "https://x.test/v1", Responses: "https://x.test/v1"}, "gpt-5.5", Chat},
		{"anthropic only", Provider{ID: "anthropic", Anthropic: "https://api.anthropic.com"}, "claude-sonnet-5", Anthropic},
		{"Copilot GPT", copilot, "gpt-5.5", Responses},
		{"Copilot Claude", copilot, "claude-sonnet-5", Anthropic},
		{"Cursor", Provider{ID: "cursor", Chat: "https://cursor.test/v1", Account: &Account{Agent: "cursor"}}, "auto", ""},
		{"Claude Code", Provider{ID: "claude", Anthropic: "https://api.anthropic.com", Account: &Account{Agent: "claude"}}, "claude-sonnet-5", ""},
		{"nothing", Provider{ID: "none"}, "m", ""},
	} {
		if got := c.p.Native(c.model); got != c.want {
			t.Errorf("%s: %q, want %q", c.name, got, c.want)
		}
	}
}
