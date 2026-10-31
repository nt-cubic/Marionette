package gateway

import (
	"bytes"
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/claudetui"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/wslrun"
)

// A Claude run is Claude Code's interactive mode on macOS, Linux and
// Windows, for this machine's Claude Code, unless the user turned it off
// or the run needs what only -p takes.
func TestClaudeTUIChosen(t *testing.T) {
	was := settings.Load()
	t.Cleanup(func() { _ = settings.Save(was) })
	claudeTUIRuns = true
	t.Cleanup(func() { claudeTUIRuns = false })
	here := claudeCLI{path: "/usr/local/bin/claude"}
	text := func(s string) []Message {
		return []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: s}}}}
	}
	prompt := []map[string]any{{"type": "text", "text": "hi"}}
	on := runtime.GOOS == "darwin" || runtime.GOOS == "linux" || runtime.GOOS == "windows"
	cases := []struct {
		name   string
		req    *Request
		binary claudeCLI
		prompt []map[string]any
		print  bool
		want   bool
		why    string // what is logged, on macOS, Linux and Windows
	}{
		{"a plain turn", &Request{Messages: text("hi")}, here, prompt, false, on, ""},
		{"turned off", &Request{Messages: text("hi")}, here, prompt, true, false, "the interactive mode is turned off"},
		{"in WSL", &Request{Messages: text("hi")}, claudeCLI{path: "claude", wsl: &wslrun.Tool{Distro: "Ubuntu", Home: "/home/me"}}, prompt, false, on, ""},
		{"in WSL, its home unknown", &Request{Messages: text("hi")}, claudeCLI{path: "claude", wsl: &wslrun.Tool{Distro: "Ubuntu"}}, prompt, false, false, "Claude Code in WSL, its home unknown"},
		{"safeguards", &Request{Messages: text("hi"), Safeguards: json.RawMessage(`{"a":1}`)}, here, prompt, false, false, "safeguards"},
		{"client cache marks", &Request{Messages: text("hi"), clientCache: &clientCache{}}, here, prompt, false, false, "the client's cache marks"},
		{"a schema", &Request{Messages: text("hi"), Format: &Format{Type: "json_schema", Schema: json.RawMessage(`{"type":"object"}`)}}, here, prompt, false, false, "a JSON schema"},
		{"an image", &Request{Messages: []Message{{Role: "user", Parts: []Part{{Kind: Image, MediaType: "image/png", Data: "AA=="}}}}}, here, prompt, false, on, ""},
		{"an image by its URL", &Request{Messages: []Message{{Role: "user", Parts: []Part{{Kind: Image, URL: "https://example.com/a.png"}}}}}, here, prompt, false, false, "an image known by its URL"},
		{"a tool's image by its URL", &Request{Messages: []Message{{Role: "user", Parts: []Part{{Kind: ToolResult, Images: []Part{{Kind: Image, URL: "https://example.com/a.png"}}}}}}}, here, prompt, false, false, "an image known by its URL"},
		{"a large image", &Request{Messages: text("hi")}, here, []map[string]any{{"type": "text", "text": "hi"}, {"type": "image", "source": map[string]any{"type": "base64", "data": strings.Repeat("A", tuiPromptMost)}}}, false, on, ""},
		{"a long first message", &Request{Messages: text("hi")}, here, []map[string]any{{"type": "text", "text": strings.Repeat("x", tuiPromptMost)}}, false, false, "a first message over 400KB"},
	}
	for _, c := range cases {
		st := settings.Load()
		st.ClaudePrintMode = c.print
		if err := settings.Save(st); err != nil {
			t.Fatal(err)
		}
		if got := claudeTUI(c.req, c.binary, c.prompt); got != c.want {
			t.Errorf("%s: claudeTUI = %v, want %v", c.name, got, c.want)
		}
		if got := claudePrintWhy(c.req, c.binary, c.prompt); on && got != c.why {
			t.Errorf("%s: -p because %q, want %q", c.name, got, c.why)
		}
	}
}

// The helper's line saying why it became -p is logged; the rest of Claude
// Code's stderr isn't.
func TestClaudeTUIFallbackLogged(t *testing.T) {
	var logs bytes.Buffer
	old := log.Writer()
	log.SetOutput(&logs)
	defer log.SetOutput(old)
	w := &lockedWriter{run: &subscriptionRun{}}
	_, _ = w.Write([]byte("warming up\nclaude-tui: Claude Code ended before it showed its prompt (exit status 1): B; this run is -p\n"))
	_, _ = w.Write([]byte("Error: something else\n"))
	got := logs.String()
	if !strings.Contains(got, "claude: Claude Code ended before it showed its prompt (exit status 1): B; this run is -p") || strings.Contains(got, "warming up") || strings.Contains(got, "something else") {
		t.Fatalf("logged %q", got)
	}
	if !strings.Contains(w.run.stderr.String(), "something else") {
		t.Fatalf("stderr kept %q", w.run.stderr.String())
	}
}

// A Claude Code in WSL is run by the helper through wsl.exe, its
// transcripts read from Windows: the distro's own folder through \\wsl$,
// or the account's config directory, a Windows folder.
func TestClaudeTUIWSL(t *testing.T) {
	c := claudeCLI{path: "claude", wsl: &wslrun.Tool{Distro: "Ubuntu", Path: "/home/me/.local/bin/claude", Home: "/home/me", Mount: "/mnt/"}}
	for _, tc := range []struct{ configDir, projects string }{
		{"", `\\wsl$\Ubuntu\home\me\.claude\projects`},
		{filepath.Join("acct", "1"), filepath.Join("acct", "1", "projects")},
	} {
		var w claudetui.WSL
		if err := json.Unmarshal([]byte(c.tuiWSL(tc.configDir)), &w); err != nil {
			t.Fatal(err)
		}
		if w.Projects != tc.projects || w.Mount != "/mnt/" {
			t.Fatalf("config %q: %+v", tc.configDir, w)
		}
		if len(w.Args) < 2 || w.Args[0] != "-d" || w.Args[1] != "Ubuntu" || w.Args[len(w.Args)-1] != c.wsl.Path {
			t.Fatalf("args %q", w.Args)
		}
	}
}

// The distro's own folder, read through \\wsl$, is a -p run's when
// Windows can't read it (no 9P redirector, as over ssh); an account's
// config directory is a Windows folder, never asked about.
func TestClaudeTUIWSLUnread(t *testing.T) {
	c := claudeCLI{wsl: &wslrun.Tool{Distro: "Ubuntu", Home: "/home/me"}}
	var asked []string
	readable := false
	old := wslStat
	wslStat = func(p string) (os.FileInfo, error) {
		asked = append(asked, p)
		if readable {
			return nil, nil
		}
		return nil, os.ErrNotExist
	}
	t.Cleanup(func() { wslStat = old })
	if got := c.tuiWSLUnread(""); got != `\\wsl$\Ubuntu\home\me` {
		t.Fatalf("unread %q", got)
	}
	readable = true
	if got := c.tuiWSLUnread(""); got != "" {
		t.Fatalf("read, yet %q", got)
	}
	readable = false
	if got := c.tuiWSLUnread("acct"); got != "" || len(asked) != 2 {
		t.Fatalf("the account's folder: %q, asked %q", got, asked)
	}
}
