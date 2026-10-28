package provider

import (
	"bufio"
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/proc"
)

// codexAppStub stands in for Codex's provider, the running app-servers
// and their ends, for codexAppStep.
type codexAppStub struct {
	on, known bool
	// route: where Codex's config sends it, beside on
	route    string
	running  []int
	listErr  error
	endErr   error
	turnOpen bool
	ended    []int
	// next: the app-server the app starts after one is ended
	next int
}

func stubCodexApp(t *testing.T) *codexAppStub {
	t.Helper()
	s := &codexAppStub{known: true, next: 100}
	oldOn, oldList, oldEnd, oldTurn := CodexAppRoute, codexAppServersOn, endCodexAppServer, codexTurnRunning
	t.Cleanup(func() {
		CodexAppRoute, codexAppServersOn, endCodexAppServer, codexTurnRunning = oldOn, oldList, oldEnd, oldTurn
		codexApp.Lock()
		codexApp.seen, codexApp.route, codexApp.stale, codexApp.stuck = false, "", nil, false
		codexApp.Unlock()
	})
	codexApp.Lock()
	codexApp.seen, codexApp.route, codexApp.stale, codexApp.stuck = false, "", nil, false
	codexApp.Unlock()
	CodexAppRoute = func() (string, bool, bool) {
		r := s.route
		if s.on {
			r += "|out"
		}
		return r, s.on, s.known
	}
	codexAppServersOn = func(context.Context, string) ([]int, error) {
		if s.listErr != nil {
			return nil, s.listErr
		}
		return slices.Clone(s.running), nil
	}
	endCodexAppServer = func(pid int) error {
		if s.endErr != nil {
			return s.endErr
		}
		s.ended = append(s.ended, pid)
		// the app starts another app-server, which reads the provider anew
		if i := slices.Index(s.running, pid); i >= 0 {
			s.running[i] = s.next
			s.next++
		}
		return nil
	}
	codexTurnRunning = func(string, time.Time) bool { return s.turnOpen }
	return s
}

// When magpie becomes Codex's provider for its used-up ChatGPT account the
// Codex app's own app-server is restarted, so the app reads its account
// again and lifts the grey on Send (Ethereal499, MasterYi), and again when
// magpie puts Codex back on the account at its reset, so the app's ChatGPT
// side comes back. Never while a Codex turn runs: one in flight through
// magpie, one ended under codexAppIdle ago (a tool between two requests),
// or one open in Codex's sessions.
func TestCodexAppServerRestartedEachWay(t *testing.T) {
	signIn(t)
	s := stubCodexApp(t)
	now := time.Now()
	idle := func() (int, time.Time) { return 0, now.Add(-time.Minute) }
	step := func(busy func() (int, time.Time)) []int { return codexAppStep(context.Background(), now, busy) }

	s.running = []int{60}
	if got := step(idle); got != nil {
		t.Fatalf("on its account with room: ended %v", got)
	}
	s.on = true
	if got := step(func() (int, time.Time) { return 1, now.Add(-time.Minute) }); got != nil {
		t.Fatalf("a Codex request in flight: ended %v", got)
	}
	if got := step(func() (int, time.Time) { return 0, now.Add(-codexAppIdle / 2) }); got != nil {
		t.Fatalf("Codex's last request just ended: ended %v", got)
	}
	s.turnOpen = true
	if got := step(idle); got != nil {
		t.Fatalf("a turn open in Codex's sessions: ended %v", got)
	}
	s.turnOpen = false
	if got := step(idle); !slices.Equal(got, []int{60}) {
		t.Fatalf("on magpie, idle: ended %v, want [60]", got)
	}
	if got := step(idle); got != nil {
		t.Fatalf("the new app-server read magpie as it started: ended %v", got)
	}
	s.on = false // the reset: back on the account
	if got := step(idle); !slices.Equal(got, []int{100}) {
		t.Fatalf("back on the account: ended %v, want [100]", got)
	}
	if got := step(idle); got != nil || CodexAppStuck() {
		t.Fatalf("after: ended %v, stuck %v", got, CodexAppStuck())
	}
}

// Disconnecting Codex takes openai_base_url out of its config, and a
// Codex app left running kept sending to 127.0.0.1:3425 (JiangLiuer222 on
// X): its app-server read the config as it started. Each change to where
// Codex's config sends it (a disconnect, a connect) restarts the app's
// app-server once no Codex turn runs, as the used-up takeover does; a
// look with nothing changed ends nothing.
func TestCodexAppServerRestartedOnDisconnect(t *testing.T) {
	signIn(t)
	s := stubCodexApp(t)
	now := time.Now()
	idle := func() (int, time.Time) { return 0, now.Add(-time.Minute) }
	step := func() []int { return codexAppStep(context.Background(), now, idle) }

	s.running = []int{60}
	s.route = "openai_base_url=http://127.0.0.1:3425/backend-api/codex"
	if got := step(); got != nil {
		t.Fatalf("first look, connected: ended %v", got)
	}
	if got := step(); got != nil {
		t.Fatalf("nothing changed: ended %v", got)
	}
	s.route = "openai_base_url=" // disconnected
	s.turnOpen = true
	if got := step(); got != nil {
		t.Fatalf("a turn open in Codex's sessions: ended %v", got)
	}
	s.turnOpen = false
	if got := step(); !slices.Equal(got, []int{60}) {
		t.Fatalf("disconnected, idle: ended %v, want [60]", got)
	}
	if got := step(); got != nil {
		t.Fatalf("the new app-server read the config as it started: ended %v", got)
	}
	s.route = "openai_base_url=http://127.0.0.1:3425/backend-api/codex" // connected again
	if got := step(); !slices.Equal(got, []int{100}) {
		t.Fatalf("connected again: ended %v, want [100]", got)
	}
}

// A magpie started while it is Codex's provider for a used-up account
// restarts the app-servers it finds: they may have started before.
// Started with Codex on its account, it leaves them be.
func TestCodexAppServerFirstLook(t *testing.T) {
	signIn(t)
	s := stubCodexApp(t)
	now := time.Now()
	idle := func() (int, time.Time) { return 0, time.Time{} }
	s.running = []int{60}
	s.on = true
	if got := codexAppStep(context.Background(), now, idle); !slices.Equal(got, []int{60}) {
		t.Fatalf("first look on magpie: ended %v", got)
	}

	s2 := stubCodexApp(t)
	s2.running = []int{70}
	if got := codexAppStep(context.Background(), now, idle); got != nil {
		t.Fatalf("first look on the account: ended %v", got)
	}
}

// Codex's provider not known (its config unreadable) changes nothing, and
// app-servers that can't be listed as the provider changes are listed at
// the next look.
func TestCodexAppServerUnknown(t *testing.T) {
	signIn(t)
	s := stubCodexApp(t)
	now := time.Now()
	idle := func() (int, time.Time) { return 0, time.Time{} }
	s.running = []int{60}
	s.known = false
	s.on = true
	if got := codexAppStep(context.Background(), now, idle); got != nil {
		t.Fatalf("not known: ended %v", got)
	}
	s.known = true
	s.listErr = errors.New("ps failed")
	if got := codexAppStep(context.Background(), now, idle); got != nil {
		t.Fatalf("not listed: ended %v", got)
	}
	s.listErr = nil
	if got := codexAppStep(context.Background(), now, idle); !slices.Equal(got, []int{60}) {
		t.Fatalf("listed at the next look: ended %v", got)
	}
}

// An app-server that outlives codexAppTries ends, or that magpie may not
// end, is left to the user: CodexAppStuck says so for the Agents row.
func TestCodexAppServerStuck(t *testing.T) {
	signIn(t)
	s := stubCodexApp(t)
	now := time.Now()
	idle := func() (int, time.Time) { return 0, time.Time{} }
	s.running = []int{60}
	s.on = true
	oldEnd := endCodexAppServer
	tries := 0
	endCodexAppServer = func(int) error { tries++; return nil } // it doesn't end
	for range codexAppTries + 2 {
		codexAppStep(context.Background(), now, idle)
	}
	if tries != codexAppTries || !CodexAppStuck() {
		t.Fatalf("tries %d (want %d), stuck %v", tries, codexAppTries, CodexAppStuck())
	}
	endCodexAppServer = oldEnd

	s.on = false // a change starts afresh
	codexAppStep(context.Background(), now, idle)
	if CodexAppStuck() {
		t.Fatal("still stuck after the provider changed")
	}
	s.on = true
	s.endErr = proc.ErrDenied
	codexAppStep(context.Background(), now, idle)
	if !CodexAppStuck() {
		t.Fatal("denied: not stuck")
	}
}

// TestCodexAppHomeHelper is TestCodexAppServersOnHome's app-server: it
// waits to be ended (a system binary's environment is kept from ps).
func TestCodexAppHomeHelper(t *testing.T) {
	if os.Getenv("MAGPIE_CODEX_HOME_HELPER") != "1" {
		return
	}
	os.Stdout.WriteString("running\n")
	time.Sleep(30 * time.Second)
}

// Only the app-servers on magpie's Codex home are restarted: one under
// another HOME or CODEX_HOME (a sandbox, another user) is left be.
func TestCodexAppServersOnHome(t *testing.T) {
	if runtime.GOOS != "darwin" && runtime.GOOS != "linux" {
		t.Skip("no environment to tell homes by here: every app-server counts")
	}
	start := func(env ...string) int {
		ctx, cancel := context.WithCancel(context.Background())
		cmd := proc.CommandContext(ctx, os.Args[0], "-test.run=^TestCodexAppHomeHelper$")
		// TMPDIR is this test's: given no testenv.Marker, the helper makes
		// a test home of its own there, which it never removes once killed
		cmd.Env = append([]string{"MAGPIE_CODEX_HOME_HELPER=1", "TMPDIR=" + t.TempDir()}, env...)
		out, _ := cmd.StdoutPipe()
		if err := cmd.Start(); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { cancel(); _ = cmd.Wait() })
		// running, so its exec is done: Start returns before Linux has
		// written the new program's environment, and /proc/<pid>/environ
		// read then is empty (#3906)
		if line, _ := bufio.NewReader(out).ReadString('\n'); line != "running\n" {
			t.Fatalf("the helper: %q", line)
		}
		return cmd.Process.Pid
	}
	ctx := context.Background()
	a, b := t.TempDir(), t.TempDir()
	onHome := start("HOME=" + a)
	if !codexAppHomeIs(ctx, onHome, filepath.Join(a, ".codex")) {
		t.Fatal("HOME's .codex: not counted")
	}
	if codexAppHomeIs(ctx, onHome, filepath.Join(b, ".codex")) {
		t.Fatal("another HOME's: counted")
	}
	onCodexHome := start("HOME="+a, "CODEX_HOME="+b)
	if !codexAppHomeIs(ctx, onCodexHome, b) {
		t.Fatal("CODEX_HOME: not counted")
	}
	if codexAppHomeIs(ctx, onCodexHome, filepath.Join(a, ".codex")) {
		t.Fatal("HOME's .codex under another CODEX_HOME: counted")
	}
	if codexAppHomeIs(ctx, 1<<30, b) {
		t.Fatal("a process that isn't there counted")
	}
}

// A turn is open in Codex's sessions when the newest task event of a file
// written to lately is task_started.
func TestCodexTurnOpen(t *testing.T) {
	home := t.TempDir()
	dir := filepath.Join(home, "sessions", "2026", "10", "27")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	write := func(name, body string, at time.Time) {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.Chtimes(p, at, at); err != nil {
			t.Fatal(err)
		}
	}
	started := `{"type":"event_msg","payload":{"type":"task_started","turn_id":"1"}}` + "\n"
	done := `{"type":"event_msg","payload":{"type":"task_complete","turn_id":"1"}}` + "\n"
	aborted := `{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"1"}}` + "\n"
	if codexTurnOpen(home, now) {
		t.Fatal("no sessions: open")
	}
	write("rollout-a.jsonl", started+done, now)
	if codexTurnOpen(home, now) {
		t.Fatal("turn complete: open")
	}
	write("rollout-b.jsonl", started+aborted, now)
	if codexTurnOpen(home, now) {
		t.Fatal("turn aborted: open")
	}
	write("rollout-c.jsonl", started+done+started, now.Add(-codexTurnWindow-time.Minute))
	if codexTurnOpen(home, now) {
		t.Fatal("a turn left open long ago: open")
	}
	write("rollout-d.jsonl", started+done+started, now)
	if !codexTurnOpen(home, now) {
		t.Fatal("a turn running: not open")
	}
}

// While the account Codex is on is near out, or out, it is read every
// minute, not five: the app greys Send on its own reading, and magpie
// takes over only on its own (Ethereal499). At the reset, as soon.
func TestCodexLookEvery(t *testing.T) {
	signIn(t) // me@example.com, active
	rememberLogins(true)
	at := func(used float64, held bool) map[string]SubscriptionQuota {
		return map[string]SubscriptionQuota{"me@example.com": {Held: held, Windows: []QuotaWindow{{Name: "5h", Used: 20}, {Name: "week", Used: used}}}}
	}
	if got := codexLookEvery(at(50, false)); got != loginSwitchEvery {
		t.Fatalf("half used: %v", got)
	}
	if got := codexLookEvery(at(91, false)); got != codexNearOutEvery {
		t.Fatalf("near out: %v", got)
	}
	if got := codexLookEvery(at(0, true)); got != codexNearOutEvery {
		t.Fatalf("out: %v", got)
	}
	if got := codexLookEvery(map[string]SubscriptionQuota{"other@example.com": {Held: true}}); got != loginSwitchEvery {
		t.Fatalf("another account out: %v", got)
	}
	if got := codexLookEvery(map[string]SubscriptionQuota{"me@example.com": {Held: true, Error: "x"}}); got != loginSwitchEvery {
		t.Fatalf("a failed read: %v", got)
	}
}

// The Agents row says until when Codex is on magpie: the latest reset of
// the windows used up.
func TestCodexOutUntil(t *testing.T) {
	signIn(t)
	rememberLogins(true)
	soon, later := time.Now().Add(time.Hour), time.Now().Add(48*time.Hour)
	u := map[string]SubscriptionQuota{"me@example.com": {Held: true, Windows: []QuotaWindow{
		{Name: "5h", Used: 100, ResetsAt: &soon},
		{Name: "week", Used: 100, ResetsAt: &later},
	}}}
	if got := codexOutUntil(u); !got.Equal(later) {
		t.Fatalf("both used up: %v, want %v", got, later)
	}
	u["me@example.com"].Windows[1].Used = 40
	if got := codexOutUntil(u); !got.Equal(soon) {
		t.Fatalf("5h used up: %v, want %v", got, soon)
	}
	q := u["me@example.com"]
	q.Held = false
	u["me@example.com"] = q
	if got := codexOutUntil(u); !got.IsZero() {
		t.Fatalf("not out: %v", got)
	}
}

// codexWindowsAppServer is the Windows Codex app's own app-server.
const codexWindowsAppServer = `C:\Users\yeton\AppData\Local\OpenAI\Codex\bin\2993767bd1dfb009\codex.exe -c features.code_mode_host=true app-server --analytics-default-enabled -c plugins.codex-app-tools@openai-bundled.mcp_servers.codex_app.enabled=true`

// Only the app's own app-server is restarted: another program's, started
// from the app's bundled codex with --listen (Miyi Helper's, seen on the
// owner's Mac), is that program's.
func TestCodexAppOwnServer(t *testing.T) {
	for _, own := range []string{
		"/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex -c features.code_mode_host=true app-server --analytics-default-enabled -c plugins.codex-app-tools@openai-bundled.mcp_servers.codex_app.enabled=true",
		// the Windows app's (OpenAI.Codex 26.1021, ChatGPT.exe's child),
		// as Windows listed it on the Windows box: run from LOCALAPPDATA,
		// not from its package (zhibukaixinyao on Discord)
		codexWindowsAppServer,
	} {
		if !isCodexAppOwnServer(own) {
			t.Errorf("the app's own app-server not taken: %s", own)
		}
	}
	for _, other := range []string{
		`C:\Users\me\AppData\Local\OpenAI\Codex\bin\2993767bd1dfb009\codex.exe app-server --listen stdio://`,
		`C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\vendor\x86_64-pc-windows-msvc\codex\codex.exe app-server`,
		`C:\Users\me\AppData\Local\Microsoft\WinGet\Links\codex.exe app-server`,
		"/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex app-server --listen stdio://",
		"/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex app-server --listen=ws://127.0.0.1:4500",
		"/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex -c forced_login_method=\"chatgpt\" exec-server --remote https://codex-cloud-environments.chatgpt.com/api",
		"/usr/local/bin/codex app-server",
	} {
		if isCodexAppOwnServer(other) {
			t.Errorf("taken for the app's own app-server: %s", other)
		}
	}
}

// Switching Codex's ChatGPT account left the Codex app on the one before
// until it was quit and opened again, and Windows has no Restart button
// for it (zhibukaixinyao on Discord): the app's app-server keeps the
// account it started with. A switch now ends that app-server once no
// Codex turn runs, as a route change does, and the app starts another on
// the new account.
func TestCodexAppServerRestartedOnAccountSwitch(t *testing.T) {
	signIn(t)
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", t.TempDir())
	s := stubCodexApp(t)
	old := listProcesses
	t.Cleanup(func() {
		listProcesses = old
		DismissCodexDaemon()
		DismissCodexApp()
	})
	listProcesses = func(context.Context) ([]proc.Process, error) {
		var ps []proc.Process
		for _, p := range s.running {
			ps = append(ps, proc.Process{PID: p, Args: codexWindowsAppServer})
		}
		return ps, nil
	}
	now := time.Now()
	idle := func() (int, time.Time) { return 0, now.Add(-time.Minute) }
	step := func() []int { return codexAppStep(context.Background(), now, idle) }

	s.running = []int{60}
	if got := step(); got != nil {
		t.Fatalf("nothing changed: ended %v", got)
	}
	noteCodexSwitch("a@x", "b@x")
	s.turnOpen = true
	if got := step(); got != nil {
		t.Fatalf("a turn open in Codex's sessions: ended %v", got)
	}
	s.turnOpen = false
	if got := step(); !slices.Equal(got, []int{60}) {
		t.Fatalf("switched, idle: ended %v, want [60]", got)
	}
	if got := step(); got != nil {
		t.Fatalf("the new app-server read the new account as it started: ended %v", got)
	}
	codexDaemonMu.Lock()
	codexDaemonChecked = time.Time{}
	codexDaemonMu.Unlock()
	if got := CodexAppStale(); got != "" {
		t.Fatalf("still said to be on %q after its restart", got)
	}
}

// The Agents row's Restart ends only the app-server the app at the bundle
// runs for itself, and leaves its window, the app itself, another
// program's --listen server and another bundle's alone (yoshi037070: a
// quit and open opened a new window).
func TestRestartCodexAppEndsOnlyItsServer(t *testing.T) {
	const bundle = "/Applications/ChatGPT.app"
	ps := []proc.Process{
		{PID: 1, Args: bundle + "/Contents/MacOS/ChatGPT"},
		{PID: 2, Args: bundle + "/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex -c features.code_mode_host=true app-server --analytics-default-enabled"},
		{PID: 3, Args: bundle + "/Contents/Resources/codex app-server --listen ws://127.0.0.1:4500"},
		{PID: 4, Args: "/Applications/Codex.app/Contents/Resources/codex app-server"},
		{PID: 5, Args: "/opt/homebrew/bin/codex app-server"},
		{PID: 6, Args: bundle + "/Contents/Resources/codex exec-server --remote x"},
	}
	oldList, oldEnd, oldWait := listProcesses, endCodexAppServer, codexAppGoneWait
	t.Cleanup(func() { listProcesses, endCodexAppServer, codexAppGoneWait = oldList, oldEnd, oldWait })
	listProcesses = func(context.Context) ([]proc.Process, error) { return slices.Clone(ps), nil }
	var ended []int
	endCodexAppServer = func(pid int) error {
		ended = append(ended, pid)
		ps = slices.DeleteFunc(ps, func(p proc.Process) bool { return p.PID == pid })
		return nil
	}
	if err := RestartCodexApp(context.Background(), bundle); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(ended, []int{2}) {
		t.Fatalf("ended %v, want only the app's own app-server 2", ended)
	}

	// none running: said
	ended = nil
	if err := RestartCodexApp(context.Background(), "/Applications/Other.app"); err == nil || len(ended) > 0 {
		t.Fatalf("no app-server: %v, ended %v", err, ended)
	}

	// one that doesn't end: said, not waited on forever
	ps = append(ps, proc.Process{PID: 7, Args: bundle + "/Contents/Resources/codex app-server"})
	endCodexAppServer = func(pid int) error { ended = append(ended, pid); return nil }
	codexAppGoneWait = 300 * time.Millisecond
	if err := RestartCodexApp(context.Background(), bundle); err == nil {
		t.Fatal("an app-server that stays: no error")
	}
}
