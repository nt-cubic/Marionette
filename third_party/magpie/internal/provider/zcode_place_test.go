package provider

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// magpie-community/plugins#86 (Hipple): ZCode's Start Plan environment
// section said a folder the agent didn't name as the user's home, "Is a git
// repository: no" whatever the folder, and magpie's own shell. The folder
// and the repository are the agent's, from its words or this computer's
// disk; what isn't known isn't said.
func TestZCodeEnvironmentSaysOnlyWhatIsKnown(t *testing.T) {
	t.Setenv("SHELL", "/bin/zsh")
	t.Setenv("ComSpec", `C:\Windows\system32\cmd.exe`)
	repo := t.TempDir()
	if err := os.Mkdir(filepath.Join(repo, ".git"), 0o755); err != nil {
		t.Fatal(err)
	}
	inRepo := filepath.Join(repo, "sub")
	if err := os.Mkdir(inRepo, 0o755); err != nil {
		t.Fatal(err)
	}
	plain := t.TempDir()
	home, _ := os.UserHomeDir()

	section := func(texts ...string) string {
		dir, git := zcodePlace(texts)
		return zcodeEnvironment("zai-api", "GLM-5.3-Flash", dir, git)
	}
	lines := func(s string) []string { return strings.Split(s, "\n")[2:] }
	cases := []struct {
		name  string
		texts []string
		want  []string // the lines after the heading, up to the platform
	}{
		{"Claude Code says it is a repository", []string{" - Primary working directory: /far/away\n - Is a git repository: true\n"},
			[]string{"- Primary working directory: /far/away", "- Is a git repository: yes"}},
		{"OpenCode says it isn't", []string{"  Working directory: /far/away\n  Is directory a git repo: no\n"},
			[]string{"- Primary working directory: /far/away", "- Is a git repository: " + zcodePrompt.Environment.GitNo}},
		{"Codex's folder in a repository on this disk", []string{"<cwd>" + inRepo + "</cwd>"},
			[]string{"- Primary working directory: " + inRepo, "- Is a git repository: yes"}},
		{"a folder on this disk outside any repository", []string{"<cwd>" + plain + "</cwd>"},
			[]string{"- Primary working directory: " + plain, "- Is a git repository: " + zcodePrompt.Environment.GitNo}},
		{"a folder not on this computer, nothing said", []string{"<cwd>/far/away</cwd>"},
			[]string{"- Primary working directory: /far/away"}},
		{"no folder at all", []string{"hello"}, nil},
	}
	for _, c := range cases {
		got := lines(section(c.texts...))
		head := got[:len(got)-3]
		if strings.Join(head, "|") != strings.Join(c.want, "|") {
			t.Errorf("%s: %q, want %q", c.name, head, c.want)
		}
		if !strings.HasPrefix(got[len(got)-3], "- Platform: ") || !strings.HasPrefix(got[len(got)-2], "- OS Version: ") {
			t.Errorf("%s: %q", c.name, got)
		}
		all := strings.Join(got, "\n")
		if strings.Contains(all, "Shell") || strings.Contains(all, "zsh") || strings.Contains(all, "cmd.exe") {
			t.Errorf("%s: magpie's shell said: %q", c.name, got)
		}
		if home != "" && c.texts[0] == "hello" && strings.Contains(all, home) {
			t.Errorf("%s: the home said as the folder: %q", c.name, got)
		}
	}
}
