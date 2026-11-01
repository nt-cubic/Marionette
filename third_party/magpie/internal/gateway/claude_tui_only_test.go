package gateway

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/claudetui"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

// tuiOnly sets the user's "Interactive only" (ClaudeTUIOnly), and
// ClaudePrintMode as print says.
func tuiOnly(t *testing.T, print bool) {
	t.Helper()
	// settings are the package's, not the test's: put back as they were
	// (cleanups run last first, so the first call's is the last)
	was := settings.Load()
	t.Cleanup(func() { _ = settings.Save(was) })
	st := settings.Load()
	st.ClaudeTUIOnly, st.ClaudePrintMode = true, print
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
}

// With "Interactive only", a request that would run as -p is refused,
// saying why, and no Claude Code starts; the interactive mode turned off
// too, -p wins.
func TestClaudeTUIOnlyRefusesPrint(t *testing.T) {
	h := newReplyHarness(t, "ok")
	// no shim: every run here would be -p (claudeMachineWhy)
	claudeTUIRuns = false
	tuiOnly(t, false)
	code, msg := h.askCode("rules", "hi")
	if code == 200 || !strings.Contains(msg, "would run as claude -p (no shim)") || !strings.Contains(msg, "only Claude Code's interactive mode") {
		t.Fatalf("%d %s", code, msg)
	}
	if runs := h.runs(); len(runs) != 0 {
		t.Fatalf("Claude Code ran: %v", runs)
	}
	tuiOnly(t, true)
	if code, msg := h.askCode("rules", "hi"); code != 200 {
		t.Fatalf("turned off, -p: %d %s", code, msg)
	}
}

// With "Interactive only", the helper is told never to become -p, and one
// that never answers initialize ends the turn with an error rather than
// a -p try; the next run tries the interactive mode again, marked broken
// or not.
func TestClaudeTUIOnlyHungHelperNoPrint(t *testing.T) {
	was := tuiInitWait
	tuiInitWait = time.Second
	t.Cleanup(func() { tuiInitWait = was })
	h, started := tuiHelperHarness(t, "exec sleep 600")
	tuiOnly(t, false)
	code, msg := h.askCode("rules", "hi")
	if code == 200 || !strings.Contains(msg, "interactive mode didn't start") {
		t.Fatalf("%d %s", code, msg)
	}
	if runs := h.runs(); len(runs) != 0 {
		t.Fatalf("a -p Claude Code ran: %v", runs)
	}
	if n := helperStarts(started); n != 1 {
		t.Fatalf("the helper started %d times, want once", n)
	}
	h.askCode("other rules", "hello")
	if n := helperStarts(started); n != 2 {
		t.Fatalf("the helper started %d times, want twice: a run after the mark went to -p", n)
	}
}

// The helper is handed OnlyEnv with "Interactive only", and not without.
func TestClaudeTUIOnlyTellsHelper(t *testing.T) {
	h := newReplyHarness(t, "ok")
	claudeTUIRuns = true
	t.Cleanup(func() { claudeTUIRuns = false })
	env := filepath.Join(t.TempDir(), "env")
	helper := filepath.Join(t.TempDir(), "magpie")
	testenv.Program(t, helper, `#!/bin/sh
echo "only=$`+claudetui.OnlyEnv+` trust=$`+claudetui.TrustEnv+`" >> '`+env+`'
shift; claude=$1; shift
exec "$claude" "$@"
`)
	claudeTUIHelper = helper
	t.Cleanup(func() { claudeTUIHelper = "" })
	h.ask("rules", "hi")
	tuiOnly(t, false)
	h.ask("other rules", "hello")
	b, _ := os.ReadFile(env)
	if got := strings.Split(strings.TrimSpace(string(b)), "\n"); len(got) != 2 || got[0] != "only= trust=1" || got[1] != "only=1 trust=1" {
		t.Fatalf("the helper was told %q", got)
	}
}

// With "Interactive only", a test's or a warm-up's "hi" is said through
// the interactive mode's helper, in stream-json, and its run ends at the
// result; one the machine can't run interactively is refused.
func TestAskClaudeTUIOnly(t *testing.T) {
	h := newReplyHarness(t, "ok")
	claudeTUIRuns = true
	t.Cleanup(func() { claudeTUIRuns = false })
	env := filepath.Join(t.TempDir(), "env")
	helper := filepath.Join(t.TempDir(), "magpie")
	testenv.Program(t, helper, `#!/bin/sh
[ "$1" = "`+claudetui.Command+`" ] || exit 9
echo "only=$`+claudetui.OnlyEnv+` trust=$`+claudetui.TrustEnv+`" >> '`+env+`'
shift; claude=$1; shift
exec "$claude" "$@"
`)
	claudeTUIHelper = helper
	t.Cleanup(func() { claudeTUIHelper = "" })
	tuiOnly(t, false)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if err := askClaude(ctx, "", "haiku"); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(env)
	if strings.TrimSpace(string(b)) != "only=1 trust=1" {
		t.Fatalf("the helper was told %q", b)
	}
	var args, told string
	for _, l := range h.runs() {
		args, _, _ = strings.Cut(l, "\n")
		told = l
	}
	for _, want := range []string{"-p --output-format stream-json --input-format stream-json --verbose --model haiku --tools  --strict-mcp-config --setting-sources "} {
		if !strings.Contains(args, want) {
			t.Fatalf("Claude Code ran with %q", args)
		}
	}
	if !strings.Contains(told, `told {"message":{"content":[{"text":"hi","type":"text"}],"role":"user"},"type":"user"}`) {
		t.Fatalf("Claude Code was told:\n%s", told)
	}
	claudeTUIRuns = false
	if err := askClaude(ctx, "", "haiku"); err == nil || !strings.Contains(err.Error(), "would run as claude -p (no shim)") {
		t.Fatalf("got %v", err)
	}
}
