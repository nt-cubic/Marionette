package provider

import (
	"bytes"
	"context"
	"errors"
	"io"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/proc"
)

// The Codex app greys its Send for every model, magpie's too, while the
// ChatGPT account it is signed in to is used up (its composer's gate,
// XP in ChatGPT.app 26.1013's app-primary: authMethod "chatgpt" and
// rate_limit.allowed false). It reads authMethod from its app-server's
// account/read, which answers no account once magpie is Codex's provider
// (requires_openai_auth false), but it asks only as the app-server
// starts. So when magpie becomes Codex's provider for the used-up
// account (the agent package's AppOut), and again when it puts Codex
// back on the account after its reset, magpie ends the app's own
// app-server, the one it started with: the app starts another at once,
// reads account/read again, and sends (tried in ChatGPT.app 26.1013 on
// macOS; its window, threads and drafts stay). It does that on every
// system the app runs on, with nothing for the user to do, once no Codex
// turn is running: none through magpie, none open in Codex's sessions.
//
// The same holds for every change magpie makes to where Codex sends its
// requests: a Disconnect that takes openai_base_url out, a Connect that
// puts it in, failover, the gateway moved to another port. The app-server
// read config.toml as it started, and a Codex app left running kept
// sending to 127.0.0.1:3425 after Codex was disconnected (JiangLiuer222 on
// X), so each such change restarts it the same way.
//
// The app-server is ended as a clean exit (proc.End): the app reads any
// other exit as a crash and shows it.

// CodexAppRoute is set by the agent package: what in Codex's config the
// app-server reads as it starts and magpie writes (its provider, base URL
// and model list: route, compared, never parsed), and whether magpie is
// Codex's provider for its used-up ChatGPT account (out, AppOut); known
// false when Codex's config can't be read. Nothing the Codex app writes
// itself (its model pick) is in route, or a pick would restart it.
var CodexAppRoute func() (route string, out, known bool)

// codexAppEvery is how often KeepCodexAppOnItsProvider looks; codexAppIdle
// how long after Codex's last request through magpie a turn counts ended
// (a tool runs between two of a turn's requests); codexAppTries how often
// one app-server is ended before magpie says it can't.
var (
	codexAppEvery = 3 * time.Second
	codexAppIdle  = 10 * time.Second
	codexAppTries = 3
)

// codexTurnWindow is how long a session file written to last counts as a
// turn maybe still running when it has one started and not ended: a turn
// whose app-server died leaves one open for good.
const codexTurnWindow = 30 * time.Minute

// codexAppListWait bounds one listing of the app-servers and their homes:
// at a load of 200 a 10s bound ran out on the owner's Mac (the change was
// taken up at the next look, 9s later).
const codexAppListWait = 30 * time.Second

// codexAppServersOn are the pids of the Codex app's own app-servers that
// run on the Codex home home; vars for tests.
var (
	codexAppServersOn = func(ctx context.Context, home string) ([]int, error) {
		ps, err := listProcesses(ctx)
		if err != nil {
			return nil, err
		}
		var out []int
		for _, p := range ps {
			if isCodexAppOwnServer(p.Args) && codexAppHomeIs(ctx, p.PID, home) {
				out = append(out, p.PID)
			}
		}
		return out, nil
	}
	endCodexAppServer = proc.End
	codexTurnRunning  = func(home string, now time.Time) bool { return codexTurnOpen(home, now) }
)

// isCodexAppOwnServer is whether a command line is the Codex app's own
// app-server, which it talks to over stdio: one another program starts
// from the app's bundled codex names where it listens (`app-server
// --listen stdio://`, Miyi Helper on macOS), and is that program's to end.
func isCodexAppOwnServer(args string) bool {
	if !isCodexAppServer(args) {
		return false
	}
	_, rest := splitExe(args)
	for _, f := range strings.Fields(rest) {
		if f == "--listen" || strings.HasPrefix(f, "--listen=") {
			return false
		}
	}
	return true
}

// codexAppHomeIs is whether app-server pid runs on Codex home home: its
// CODEX_HOME, else its HOME's .codex. A system that doesn't tell
// (Windows) counts as yes: one Codex home is the rule there.
func codexAppHomeIs(ctx context.Context, pid int, home string) bool {
	env, err := proc.EnvOf(ctx, pid, "CODEX_HOME", "HOME")
	if errors.Is(err, proc.ErrNoEnv) {
		return true
	}
	if err != nil {
		return false
	}
	h := env["CODEX_HOME"]
	if h == "" && env["HOME"] != "" {
		h = filepath.Join(env["HOME"], ".codex")
	}
	return h != "" && sameDir(h, home)
}

// sameDir is whether a and b name one folder, through links too.
func sameDir(a, b string) bool {
	a, b = filepath.Clean(a), filepath.Clean(b)
	if a == b {
		return true
	}
	ra, err1 := filepath.EvalSymlinks(a)
	rb, err2 := filepath.EvalSymlinks(b)
	return err1 == nil && err2 == nil && ra == rb
}

// codexApp is what KeepCodexAppOnItsProvider knows.
var codexApp struct {
	sync.Mutex
	seen  bool
	route string
	// stale: the app-servers started before Codex's provider last changed,
	// each with how often it was ended
	stale map[int]int
	// stuck: one of them outlived codexAppTries ends, or may not be ended
	stuck bool
}

// CodexAppStuck is whether a Codex app is left on the provider it started
// with, which magpie couldn't restart: the user quits and opens it.
func CodexAppStuck() bool {
	c := &codexApp
	c.Lock()
	defer c.Unlock()
	return c.stuck
}

// codexAppAccountLeft has the Codex app's app-server pid, left on the
// ChatGPT account Codex was switched away from (noteCodexSwitch), ended
// as a route change's are, once no Codex turn runs: the app starts
// another, which reads auth.json and the new account afresh, so the app
// shows that account's limits and sends on it with no quit and reopen
// (zhibukaixinyao on Discord: Windows has no Restart button). One
// switched away and back before then is restarted all the same, which
// costs the app nothing it shows.
func codexAppAccountLeft(pid int) {
	c := &codexApp
	c.Lock()
	defer c.Unlock()
	if c.stale == nil {
		c.stale = map[int]int{}
	}
	if _, ok := c.stale[pid]; !ok {
		c.stale[pid] = 0
	}
}

// KeepCodexAppOnItsProvider runs codexAppStep every codexAppEvery till
// ctx ends. busy says how many of Codex's requests are in flight through
// magpie and when the last ended.
func KeepCodexAppOnItsProvider(ctx context.Context, busy func() (int, time.Time)) {
	t := time.NewTicker(codexAppEvery)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-t.C:
			codexAppStep(ctx, now, busy)
		}
	}
}

// codexAppStep notes the app-servers running as Codex's route changes
// (magpie becomes Codex's provider for the used-up account, or stops being
// it; Codex is connected or disconnected), and ends each of them once no
// Codex turn is running. Started with magpie on, the app may
// have opened before: its app-servers are ended too. It says which pids it
// ended.
func codexAppStep(ctx context.Context, now time.Time, busy func() (int, time.Time)) []int {
	if CodexAppRoute == nil {
		return nil
	}
	route, out, known := CodexAppRoute()
	if !known {
		return nil
	}
	home := codexHome()
	c := &codexApp
	c.Lock()
	// at the first look only a used-up account's app-servers are ended:
	// what the others read before magpie started isn't known
	changed := !c.seen && out || c.seen && c.route != route
	if !changed {
		c.seen, c.route = true, route
	}
	c.Unlock()
	if changed {
		lctx, cancel := context.WithTimeout(ctx, codexAppListWait)
		pids, err := codexAppServersOn(lctx, home)
		cancel()
		if err != nil {
			// unknown: the change is taken up again at the next look
			log.Printf("codex: listing the Codex app's app-servers: %v", err)
			return nil
		}
		c.Lock()
		c.seen, c.route = true, route
		c.stale, c.stuck = map[int]int{}, false
		for _, p := range pids {
			c.stale[p] = 0
		}
		c.Unlock()
	}
	c.Lock()
	n := len(c.stale)
	c.Unlock()
	if n == 0 {
		return nil
	}
	lctx, cancel := context.WithTimeout(ctx, codexAppListWait)
	pids, err := codexAppServersOn(lctx, home)
	cancel()
	if err != nil {
		return nil
	}
	running := map[int]bool{}
	for _, p := range pids {
		running[p] = true
	}
	c.Lock()
	for p := range c.stale {
		if !running[p] {
			delete(c.stale, p) // it read the provider afresh as it started
		}
	}
	n = len(c.stale)
	c.Unlock()
	if n == 0 {
		return nil
	}
	if inFlight, last := busy(); inFlight > 0 || now.Sub(last) < codexAppIdle {
		return nil
	}
	if codexTurnRunning(home, now) {
		return nil
	}
	var ended []int
	c.Lock()
	defer c.Unlock()
	for p, tries := range c.stale {
		if tries >= codexAppTries {
			c.stuck = true
			continue
		}
		c.stale[p] = tries + 1
		if err := endCodexAppServer(p); err != nil {
			log.Printf("codex: restarting the Codex app's app-server %d on its provider: %v", p, err)
			if errors.Is(err, proc.ErrDenied) {
				c.stale[p], c.stuck = codexAppTries, true
			}
			continue
		}
		ended = append(ended, p)
	}
	return ended
}

// codexTurnOpen is whether a Codex session under home has a turn started
// and not ended (task_started after its last task_complete or
// turn_aborted) in a file written to within codexTurnWindow of now: the
// app-server running it would cut it short.
func codexTurnOpen(home string, now time.Time) bool {
	open := false
	_ = filepath.WalkDir(filepath.Join(home, "sessions"), func(path string, d fs.DirEntry, err error) error {
		if err != nil || open {
			if open {
				return fs.SkipAll
			}
			return nil
		}
		if d.IsDir() || !strings.HasSuffix(d.Name(), ".jsonl") {
			return nil
		}
		info, err := d.Info()
		if err != nil || now.Sub(info.ModTime()) > codexTurnWindow {
			return nil
		}
		open = sessionTurnOpen(path)
		return nil
	})
	return open
}

// sessionTurnTail is how much of a session file's end sessionTurnOpen reads.
const sessionTurnTail = 512 << 10

// sessionTurnOpen is whether the session file at path ends inside a turn.
// A file that can't be read counts as open: a turn cut short costs more
// than an app greyed a few seconds more.
func sessionTurnOpen(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return !errors.Is(err, fs.ErrNotExist)
	}
	defer f.Close()
	if info, err := f.Stat(); err == nil && info.Size() > sessionTurnTail {
		if _, err := f.Seek(info.Size()-sessionTurnTail, io.SeekStart); err != nil {
			return true
		}
	}
	b, err := io.ReadAll(f)
	if err != nil {
		return true
	}
	started := bytes.LastIndex(b, []byte(`"type":"task_started"`))
	ended := max(bytes.LastIndex(b, []byte(`"type":"task_complete"`)), bytes.LastIndex(b, []byte(`"type":"turn_aborted"`)))
	return started > ended
}

// codexAppGoneWait is how long RestartCodexApp waits for the app-servers
// it ended to be gone; a var for tests.
var codexAppGoneWait = 10 * time.Second

// RestartCodexApp restarts the app-server the Codex desktop app at bundle
// (/Applications/ChatGPT.app) runs for itself, as codexAppStep does: it is
// ended as a clean exit, and the app starts another, which reads Codex's
// config and model list afresh, while the app's window, threads and drafts
// stay as they are. The Agents row's Restart quit the app and opened it
// again, which opened a new window (yoshi037070 on Discord, 10-27). An
// app-server another program runs from the bundle (--listen) is left
// alone. It waits until the ones ended are gone, and errs when none ran.
func RestartCodexApp(ctx context.Context, bundle string) error {
	ps, err := listProcesses(ctx)
	if err != nil {
		return err
	}
	in := strings.TrimSuffix(bundle, "/") + "/Contents/"
	var ended []int
	for _, p := range ps {
		if !strings.HasPrefix(strings.TrimSpace(p.Args), in) || !isCodexAppOwnServer(p.Args) {
			continue
		}
		if err := endCodexAppServer(p.PID); err != nil {
			return err
		}
		ended = append(ended, p.PID)
	}
	if len(ended) == 0 {
		return errors.New("the Codex app's app-server isn't running")
	}
	end := time.Now().Add(codexAppGoneWait)
	for {
		ps, err := listProcesses(ctx)
		if err == nil && !slices.ContainsFunc(ps, func(p proc.Process) bool { return slices.Contains(ended, p.PID) }) {
			return nil
		}
		if time.Now().After(end) {
			return errors.New("the Codex app's app-server didn't end")
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(200 * time.Millisecond):
		}
	}
}
