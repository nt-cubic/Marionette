package gateway

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/claudetui"
	"github.com/yetone/magpie/internal/testenv"
)

// tuiHelperHarness is a script harness whose runs in Claude Code's
// interactive mode start a stand-in for magpie's claude-tui-helper: it
// notes each start in started, waits as hold says (a shell command), and
// becomes the -p Claude Code it was handed, as the helper does when the
// interactive Claude Code doesn't show its prompt in time.
func tuiHelperHarness(t *testing.T, hold string) (*resumeHarness, string) {
	t.Helper()
	h := newReplyHarness(t, "ok")
	claudeTUIRuns = true
	t.Cleanup(func() { claudeTUIRuns = false })
	started := filepath.Join(t.TempDir(), "started")
	helper := filepath.Join(t.TempDir(), "magpie")
	testenv.Program(t, helper, `#!/bin/sh
[ "$1" = "`+claudetui.Command+`" ] || exit 9
echo start >> '`+started+`'
`+hold+`
shift; claude=$1; shift
exec "$claude" "$@"
`)
	claudeTUIHelper = helper
	t.Cleanup(func() { claudeTUIHelper = "" })
	return h, started
}

func helperStarts(path string) int {
	b, _ := os.ReadFile(path)
	return strings.Count(string(b), "start")
}

// An interactive Claude Code that takes longer than a control request's
// 10s to show its prompt is waited for: the bridge gave the helper 10s to
// answer initialize and ended it as it waited, before it became -p on its
// own (claudetui.ReadyWait, 45s), and every try was answered 502 "Claude
// Code has not answered magpie's initialize request in 10s" (#4504,
// derrors: 2 vCPU, Claude Code in Docker).
func TestClaudeTUISlowStartAnswered(t *testing.T) {
	h, started := tuiHelperHarness(t, "sleep 12")
	h.ask("rules", "hi") // fails the test on anything but 200
	if n := helperStarts(started); n != 1 {
		t.Fatalf("the helper started %d times, want once", n)
	}
	if _, ok := claudetui.BrokenSince(); ok {
		t.Fatal("a slow start that answered was marked broken")
	}
}

// A helper that never answers initialize is let go, the machine is marked
// as one whose interactive mode doesn't start, and the turn is answered by
// -p: the turn, and the next, never wait on the interactive mode again.
func TestClaudeTUIHungHelperGoesPrint(t *testing.T) {
	was := tuiInitWait
	tuiInitWait = time.Second
	t.Cleanup(func() { tuiInitWait = was })
	h, started := tuiHelperHarness(t, "exec sleep 600")
	h.ask("rules", "hi")
	if n := helperStarts(started); n != 1 {
		t.Fatalf("the helper started %d times, want once", n)
	}
	if _, ok := claudetui.BrokenSince(); !ok {
		t.Fatal("a helper that never answered left no mark")
	}
	h.ask("other rules", "hello")
	if n := helperStarts(started); n != 1 {
		t.Fatalf("the helper started %d times, want once: a run after the mark went to the interactive mode", n)
	}
}
