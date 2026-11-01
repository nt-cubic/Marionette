package gateway

import (
	"testing"
)

// The provider editor's row says how the last run went: the interactive
// mode, -p with claudeTUI's code, or -p after the helper couldn't start
// the interactive mode.
func TestClaudeModeLast(t *testing.T) {
	claudeLast.Lock()
	before := claudeLast.run
	claudeLast.Unlock()
	t.Cleanup(func() { claudeLast.Lock(); claudeLast.run = before; claudeLast.Unlock() })

	noteClaudeRun("schema")
	if m := ClaudeModeNow(); m.Last == nil || m.Last.TUI || m.Last.Why != "schema" || m.Last.At.IsZero() {
		t.Fatalf("after a schema run: %+v", m.Last)
	}
	noteClaudeRun("")
	if m := ClaudeModeNow(); m.Last == nil || !m.Last.TUI || m.Last.Why != "" {
		t.Fatalf("after an interactive run: %+v", m.Last)
	}
	// the helper becoming -p after it started is the last run's mode too
	w := &lockedWriter{run: &subscriptionRun{}}
	_, _ = w.Write([]byte("claude-tui: Claude Code ended before it showed its prompt (exit status 1): B; this run is -p\n"))
	if m := ClaudeModeNow(); m.Last == nil || m.Last.TUI || m.Last.Why != "start" {
		t.Fatalf("after the helper became -p: %+v", m.Last)
	}
}

// claudeTUI's code names what sent a run to -p, as a word the GUI
// translates.
func TestClaudeTUICodes(t *testing.T) {
	was := claudeTUIRuns
	claudeTUIRuns = true
	t.Cleanup(func() { claudeTUIRuns = was })
	req := &Request{Messages: []Message{{Role: "user", Parts: []Part{{Kind: Text, Text: "hi"}}}}, Safeguards: []byte(`{"a":1}`)}
	if tui, code := claudeTUI(req, claudeCLI{path: "/usr/local/bin/claude"}, []map[string]any{{"type": "text", "text": "hi"}}); tui || code != "safeguards" {
		t.Fatalf("with safeguards: %v %q", tui, code)
	}
	if tui, code := claudeTUI(req, claudeCLI{}, nil); tui || code != "missing" {
		t.Fatalf("no Claude Code: %v %q", tui, code)
	}
}
