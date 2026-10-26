package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// claudeWorkDirs runs two Claude subscription requests through a script
// standing in for Claude Code, with a temp folder of the test's own and a
// home kept in git, and gives the folders the two runs worked in and the
// folder they should share.
func claudeWorkDirs(t *testing.T, prepare func(work string)) (dirs []string, work string) {
	t.Helper()
	dirs, work, _, _, _ = claudeWorkDirsTold(t, prepare, func(string) string { return "rules" })
	return dirs, work
}

// claudeWorkDirsTold is claudeWorkDirs with the caller's instructions
// system gives, from the test's home; it gives the gateway too, and the
// environment the runs had.
func claudeWorkDirsTold(t *testing.T, prepare func(work string), system func(home string) string) (dirs []string, work string, s *Server, env, told string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		t.Skip("a shell script stands in for Claude Code")
	}
	home := t.TempDir()
	if err := os.Mkdir(filepath.Join(home, ".git"), 0o700); err != nil {
		t.Fatal(err)
	}
	testenv.SetHome(t, home)
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	t.Setenv("TMPDIR", t.TempDir())
	work = filepath.Join(os.TempDir(), claudeWorkName())
	if prepare != nil {
		prepare(work)
	}
	dir := t.TempDir()
	script := `#!/bin/sh
pwd -P >> ` + dir + `/dirs
env >> ` + dir + `/env
while read -r line; do
  echo "$line" >> ` + dir + `/told
  echo '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m","model":"claude-sonnet-5","usage":{"input_tokens":1}}}}'
  echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}}'
  echo '{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}}'
  echo '{"type":"stream_event","event":{"type":"message_stop"}}'
  echo '{"type":"result","subtype":"success","is_error":false,"result":""}'
done
`
	testenv.Program(t, filepath.Join(dir, "claude"), script)
	t.Setenv("PATH", dir+string(os.PathListSeparator)+os.Getenv("PATH"))

	s = New()
	p := provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude", User: "u"}}
	rules, _ := json.Marshal(system(home))
	for _, said := range []string{"one", "two"} {
		body := `{"model":"claude-sonnet-5","max_tokens":100,"system":` + string(rules) + `,"messages":[{"role":"user","content":"` + said + `"}]}`
		rec := httptest.NewRecorder()
		var u Usage
		if code, msg := s.serveClaudeSubscription(rec, httptest.NewRequest("POST", "/v1/messages", strings.NewReader(body)), provider.Anthropic, p, "claude-sonnet-5", []byte(body), &u); code != 200 {
			t.Fatalf("%d %s", code, msg)
		}
	}
	b, _ := os.ReadFile(filepath.Join(dir, "dirs"))
	e, _ := os.ReadFile(filepath.Join(dir, "env"))
	m, _ := os.ReadFile(filepath.Join(dir, "told"))
	return strings.Fields(string(b)), evalSymlinks(work), s, string(e), string(m)
}

// Every run works in the same folder: Claude Code puts its working
// directory in the system prompt, ahead of the conversation, so a folder of
// each run's own left nothing past Claude Code's own part of the prompt to
// be read from the cache. The folder is the user's own, made for them
// alone, in the temp folder: not under a home kept in git, whose status
// Claude Code would put in the prompt.
func TestClaudeRunsShareAWorkDir(t *testing.T) {
	dirs, work := claudeWorkDirs(t, nil)
	if len(dirs) != 2 || dirs[0] != work || dirs[1] != work {
		t.Fatalf("working directories: %q, want %s", dirs, work)
	}
	if home := evalSymlinks(os.Getenv("HOME")); strings.HasPrefix(work, home+string(filepath.Separator)) || !strings.HasPrefix(work, evalSymlinks(os.TempDir())+string(filepath.Separator)) {
		t.Fatalf("work folder %s: in the home %s, or not in the temp folder", work, home)
	}
	fi, err := os.Stat(work)
	if err != nil || fi.Mode().Perm() != 0o700 {
		t.Fatalf("work folder: %v %v", fi.Mode(), err)
	}
}

// A work folder others can write to is not worked in, as anything put
// there (a CLAUDE.md) would be read by every run: each run works in a
// folder of its own instead.
func TestClaudeWorkDirOthersCanWriteIsAvoided(t *testing.T) {
	dirs, work := claudeWorkDirs(t, func(work string) {
		if err := os.MkdirAll(work, 0o700); err != nil {
			t.Fatal(err)
		}
		os.Chmod(work, 0o777)
	})
	if len(dirs) != 2 || dirs[0] == work || dirs[1] == work || dirs[0] == dirs[1] {
		t.Fatalf("working directories: %q, shared %s", dirs, work)
	}
}

// A symlink where the work folder goes, to a folder anyone could have
// filled, is not followed: each run works in a folder of its own.
func TestClaudeWorkDirSymlinkIsAvoided(t *testing.T) {
	var target string
	dirs, work := claudeWorkDirs(t, func(work string) {
		target = t.TempDir()
		if err := os.Symlink(target, work); err != nil {
			t.Fatal(err)
		}
	})
	target = evalSymlinks(target)
	if len(dirs) != 2 || dirs[0] == target || dirs[1] == target || dirs[0] == dirs[1] {
		t.Fatalf("working directories: %q, linked %s (%s)", dirs, target, work)
	}
}

// A caller that says which folder it works in has Claude Code run there:
// Claude Code tells its model the folder it runs in as the working
// directory, and a model told magpie's own folder wrote the caller's files
// into it (a pelican.html left in magpie-claude-work), through tools that
// run in the caller's. Each client's way of saying it is read, the last
// said counting. The caller's folder keeps sessions of its own, which are
// never swept, and its repository's status is not added to the prompt.
func TestClaudeRunsInTheCallersWorkDir(t *testing.T) {
	for _, c := range []struct{ name, said string }{
		{"Claude Code", "# Environment\nYou have been invoked in the following environment: \n - Primary working directory: %s\n - Is a git repository: true\n"},
		{"Codex", "<environment_context>\n  <cwd>%s</cwd>\n  <shell>zsh</shell>\n</environment_context>"},
		{"OpenCode", "<env>\n  Working directory: %s\n  Is directory a git repo: yes\n</env>"},
		{"pi", "Current date: 2026-10-26\nCurrent working directory: %s"},
		{"said twice", " - Primary working directory: /nowhere/at/all\nlater:\n - Primary working directory: %s"},
	} {
		t.Run(c.name, func(t *testing.T) {
			var proj, old string
			dirs, work, s, env, told := claudeWorkDirsTold(t, nil, func(home string) string {
				proj = filepath.Join(home, "proj")
				if err := os.MkdirAll(proj, 0o755); err != nil {
					t.Fatal(err)
				}
				// a session of the user's own in that folder, older than
				// any the bridge keeps
				sessions := claudeSessions("", evalSymlinks(proj))
				old = filepath.Join(sessions[0], "mine.jsonl")
				if err := os.MkdirAll(sessions[0], 0o700); err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(old, []byte("{}\n"), 0o600); err != nil {
					t.Fatal(err)
				}
				then := time.Now().Add(-48 * time.Hour)
				_ = os.Chtimes(old, then, then)
				return strings.ReplaceAll(c.said, "%s", proj)
			})
			proj = evalSymlinks(proj)
			if len(dirs) != 2 || dirs[0] != proj || dirs[1] != proj {
				t.Fatalf("working directories: %q, want the caller's %s (not %s)", dirs, proj, work)
			}
			for _, d := range claudeSessions("", proj) {
				if _, swept := s.subscription.swept.Load(d); swept {
					t.Fatalf("the caller's sessions in %s were swept", d)
				}
			}
			if _, err := os.Stat(old); err != nil {
				t.Fatalf("the user's own session: %v", err)
			}
			if !strings.Contains(env, "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS=1") {
				t.Fatal("Claude Code would add the repository's status to the prompt")
			}
			if strings.Contains(told, "every tool runs in") {
				t.Fatalf("a run in the caller's own folder was told its environment is elsewhere: %s", told)
			}
		})
	}
}

// A folder the caller names that isn't one here (a caller on another
// computer), or isn't absolute, leaves the runs in magpie's own folder.
// Claude Code's prompt then gives this computer's platform and folder, and
// the caller's another's, so the run is told after the caller's
// instructions that theirs is where the tools run (vv_nil on X: a Mac
// client of a Linux gateway spent tool calls finding out which is real).
// A caller that names no folder is told nothing more.
func TestClaudeCallersWorkDirElsewhereIsNotUsed(t *testing.T) {
	for _, c := range []struct {
		said      string
		elsewhere bool
	}{
		{" - Primary working directory: /no/such/folder/here", true},
		{" - Primary working directory: relative/proj", true},
		{"<cwd>C:\\Users\\someone\\proj</cwd>", true},
		{"no folder named", false},
	} {
		dirs, work, _, _, told := claudeWorkDirsTold(t, nil, func(string) string { return c.said })
		if len(dirs) != 2 || dirs[0] != work || dirs[1] != work {
			t.Fatalf("%q: working directories %q, want %s", c.said, dirs, work)
		}
		if got := strings.Count(told, "every tool runs in"); got != 2*btoi(c.elsewhere) {
			t.Fatalf("%q: told the environment is elsewhere in %d of 2 runs, want elsewhere=%v: %s", c.said, got, c.elsewhere, told)
		}
		if c.elsewhere && strings.Index(told, "every tool runs in") > strings.Index(told, "/external_system_instructions") {
			t.Fatalf("%q: the note is not with the caller's instructions: %s", c.said, told)
		}
	}
}

func btoi(b bool) int {
	if b {
		return 1
	}
	return 0
}
