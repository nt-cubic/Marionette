package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// The fixtures are requests the agents sent a fake upstream, run in a
// sandbox home from a folder named "a&b <c> 中文" (Claude Code 2.1.296,
// Codex 0.162.0) or "My Proj" (Pi 1.1.0); the sandbox's path is replaced
// with /Users/me/work.
func fixture(t *testing.T, name string) []byte {
	t.Helper()
	b, err := os.ReadFile(filepath.Join("testdata", "workdir", name))
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// Each agent's folder is read from its own request, entities and all.
func TestWorkDirOfEachAgent(t *testing.T) {
	for _, tc := range []struct {
		file  string
		parse func([]byte) (*Request, error)
		want  string
	}{
		{"claude-code-2.1.296.json", parseAnthropic, "/Users/me/work/proj/a&b <c> 中文"},
		{"codex-0.162.0.json", parseResponses, "/Users/me/work/proj/a&b <c> 中文"},
		{"pi-1.1.0.json", parseChat, "/Users/me/work/proj/My Proj"},
	} {
		req, err := tc.parse(fixture(t, tc.file))
		if err != nil {
			t.Fatal(tc.file, err)
		}
		if got := workDir(req); got != tc.want {
			t.Errorf("%s: %q, want %q", tc.file, got, tc.want)
		}
	}
}

// OpenCode 2.0.20's environment, as its source writes it (its run waits on
// a background service a sandbox doesn't start): in the system prompt,
// and when it changes, after.
func TestWorkDirOpenCode(t *testing.T) {
	env := func(dir string) string {
		return strings.Join([]string{"<env>", "  Working directory: " + dir, "  Workspace root folder: /Users/me", "  Is directory a git repo: yes", "  Platform: darwin",
			"  Prefer /tmp/x over generic system temporary directories such as /tmp; it is pre-created and approved for external access.", "</env>"}, "\n")
	}
	req := &Request{System: "You are opencode.\nHere is some useful information about the environment you are running in:\n" + env("/Users/me/acme"),
		Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: "hi"}}}}}
	if got := workDir(req); got != "/Users/me/acme" {
		t.Errorf("%q", got)
	}
	req.Messages = append(req.Messages, Message{Role: "assistant", Parts: []Part{{Kind: Text, Text: "ok"}}},
		Message{Role: "user", Parts: []Part{{Kind: Text, Text: "The environment you are running in is now:\n" + env("/Users/me/oss")}}})
	if got := workDir(req); got != "/Users/me/oss" {
		t.Errorf("changed: %q", got)
	}
}

// Claude Code moved to another folder says so after, with the one it was
// in (its source: "Primary working directory: /y (was /x)"); the last
// said goes. What the model or a tool said, or a "Working directory:" out
// of OpenCode's <env>, isn't the agent's folder; nothing said is unknown.
func TestWorkDirLastSaidAndOnlyByTheAgent(t *testing.T) {
	req, err := parseAnthropic(fixture(t, "claude-code-2.1.296.json"))
	if err != nil {
		t.Fatal(err)
	}
	req.Messages = append(req.Messages,
		Message{Role: "assistant", Parts: []Part{{Kind: Text, Text: " - Primary working directory: /from/the/model"}}},
		Message{Role: "user", Parts: []Part{{Kind: ToolResult, Text: "<cwd>/from/a/tool</cwd>"}, {Kind: Text, Text: "<system-reminder>\nPrimary working directory: /Users/me/next (was /Users/me/work/proj/a&b &lt;c&gt; 中文)\n</system-reminder>"}}},
	)
	if got := workDir(req); got != "/Users/me/next" {
		t.Errorf("%q", got)
	}
	for _, text := range []string{"hi", "Working directory: /x", "the <cwd></cwd> tag"} {
		if got := workDir(&Request{System: "You are an agent.", Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: text}}}}}); got != "" {
			t.Errorf("%q: %q", text, got)
		}
	}
}

// The agents' own requests go by a group's rule by folder, each over its
// own protocol: the folder under the rule's to its member, the trace
// telling the folder, the rule's condition and that it matched; one out
// of it, or with no folder said, by the group's order.
func TestRuleByFolder(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fixtures' folders are Unix paths")
	}
	s, _, _ := ruled(t, provider.Rule{Use: "b/big", Folders: []string{"/Users/me/work/proj"}})
	group := func(body []byte) string {
		var m map[string]any
		if err := json.Unmarshal(body, &m); err != nil {
			t.Fatal(err)
		}
		m["model"], m["stream"] = "group/r", false
		delete(m, "thinking")
		delete(m, "reasoning")
		b, _ := json.Marshal(m)
		return string(b)
	}
	for _, tc := range []struct {
		name, path, body, folder string
		n                        int
	}{
		{"claude code", "/v1/messages", group(fixture(t, "claude-code-2.1.296.json")), "/Users/me/work/proj/a&b <c> 中文", 1},
		{"codex", "/v1/responses", group(fixture(t, "codex-0.162.0.json")), "/Users/me/work/proj/a&b <c> 中文", 1},
		{"pi", "/v1/chat/completions", group(fixture(t, "pi-1.1.0.json")), "/Users/me/work/proj/My Proj", 1},
		{"pi elsewhere", "/v1/chat/completions", strings.ReplaceAll(group(fixture(t, "pi-1.1.0.json")), "/Users/me/work/proj/", "/Users/me/work/project/"), "/Users/me/work/project/My Proj", 0},
		{"no folder", "/v1/chat/completions", chat("hi", nil, 0, ""), "", 0},
	} {
		rec := httptest.NewRecorder()
		req := httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body))
		req.Header.Set("x-session-id", tc.name)
		s.Handler().ServeHTTP(rec, req)
		want := map[int]string{0: "from ka", 1: "from kb"}[tc.n]
		r := lastRoute(s)
		if rec.Code != 200 || !strings.Contains(rec.Body.String(), want) {
			t.Errorf("%s: %d %.300s", tc.name, rec.Code, rec.Body.String())
			continue
		}
		if r.Rule == nil || r.Rule.N != tc.n || r.Rule.Folder != tc.folder {
			t.Errorf("%s: rule %+v", tc.name, r.Rule)
			continue
		}
		if tc.n == 1 && (len(r.Rule.When) != 1 || r.Rule.When[0] != "folder /Users/me/work/proj") {
			t.Errorf("%s: when %q", tc.name, r.Rule.When)
		}
	}
}
