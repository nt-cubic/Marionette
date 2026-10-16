package gateway

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
)

// magpie-community/plugins#86: a Go request to /alpha/generate said the
// user's home as the working directory, outside a git repository, whatever
// folder the agent worked in. It says the agent's folder and repository,
// from the agent's words or this computer's disk; a request that names no
// folder keeps the CLI's config in the home.
func TestCommandCodeGoSaysTheAgentsFolder(t *testing.T) {
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	plain := t.TempDir()
	home, _ := os.UserHomeDir()
	ask := func(system string) gjson.Result {
		t.Helper()
		body, _ := json.Marshal(map[string]any{
			"model": "m", "max_tokens": 16, "system": system,
			"messages": []any{map[string]any{"role": "user", "content": "hi"}},
		})
		req, err := parse(provider.Anthropic, body)
		if err != nil {
			t.Fatal(err)
		}
		return gjson.GetBytes(cmdRequest(req, "m"), "config")
	}
	for _, c := range []struct {
		name, system, dir string
		git               bool
	}{
		{"Claude Code says it is a repository", "Primary working directory: /far/away\nIs a git repository: true\n", "/far/away", true},
		{"Claude Code says it isn't", "Primary working directory: /far/away\nIs a git repository: false\n", "/far/away", false},
		{"a repository on this disk", "<cwd>" + repo + "</cwd>", repo, true},
		{"a folder on this disk outside one", "<cwd>" + plain + "</cwd>", plain, false},
		{"a folder not here, nothing said", "<cwd>/far/away</cwd>", "/far/away", false},
		{"no folder", "You are helpful.", home, false},
	} {
		got := ask(c.system)
		if got.Get("workingDir").String() != c.dir || got.Get("isGitRepo").Bool() != c.git {
			t.Errorf("%s: %s, want %s %v", c.name, got.Raw, c.dir, c.git)
		}
	}
}
