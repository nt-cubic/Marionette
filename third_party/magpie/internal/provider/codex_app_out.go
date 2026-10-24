package provider

import (
	"context"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// The Codex app reads its sign-in only as it opens: an account used up
// while it runs greys its Send for every model, magpie's included, and
// stays so after magpie becomes Codex's provider (MasterYi on Discord),
// till the app is quit and opened again. What follows tells the account
// is out as soon as a reply says so, not at the next look five minutes
// on, and keeps which Codex app was open when it ran out, for the GUI to
// reopen (once) or name.

// codexLiveChecks counts the running loops that keep Codex on an account
// with room (KeepOnAnAccountWithRoom) past their first reading: while one
// does, a reply that says Codex's account is out has it read again at
// once (CodexMayBeUsedUp). A test's gateway, gone within the minute
// before that first reading, never turns it on, so tests never ask the
// vendor.
var codexLiveChecks atomic.Int32

// codexRecheckEvery is the least time between two such readings.
var codexRecheckEvery = 20 * time.Second

var codexRecheck struct {
	sync.Mutex
	at time.Time
}

// codexReadNow reads whether Codex's account is out; a var for tests.
var codexReadNow = func(user string) {
	StaleAllowance("codex", user)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	noteCodexUsedUp(CodexUsedUp(ctx))
}

// CodexMayBeUsedUp is a reply's word that user's ChatGPT account may be
// out (usage_limit_reached, or a window heard at 100%): for the account
// Codex is signed in to, its allowance is read again at once, at most
// every codexRecheckEvery, and a change told as the five-minute look tells
// it (noteCodexUsedUp).
func CodexMayBeUsedUp(user string) {
	if codexLiveChecks.Load() <= 0 || user == "" {
		return
	}
	active := ""
	for _, l := range Logins("codex") {
		if l.Active && strings.EqualFold(l.User, user) {
			active = l.User
		}
	}
	if active == "" {
		return
	}
	c := &codexRecheck
	c.Lock()
	if time.Since(c.at) < codexRecheckEvery {
		c.Unlock()
		return
	}
	c.at = time.Now()
	c.Unlock()
	go codexReadNow(active)
}

// codexAppOut is the Codex app open when the account it is signed in to
// ran out: its app-server's pid, and whether the GUI tried reopening it.
var codexAppOut struct {
	sync.Mutex
	user    string
	pid     int
	tried   bool
	checked time.Time
}

// noteCodexAppOut keeps, as the account Codex is signed in to runs out
// (out) or has room again, which Codex app was open then.
func noteCodexAppOut(out bool) {
	user, pid := "", 0
	if out {
		for _, l := range Logins("codex") {
			if l.Active {
				user = l.User
			}
		}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		ps, err := listProcesses(ctx)
		cancel()
		if err == nil {
			pid = findCodexApp(ps)
		}
	}
	c := &codexAppOut
	c.Lock()
	defer c.Unlock()
	if pid == 0 {
		// none open: one opened from now reads the sign-in afresh
		c.user, c.pid, c.tried = "", 0, false
		return
	}
	c.user, c.pid, c.tried, c.checked = user, pid, false, time.Now()
}

// CodexAppOut is the ChatGPT account of a Codex app that was open when
// that account ran out and hasn't been opened again since, "" for none:
// it sends nothing till it is (the agent package's AppOut says whether
// magpie is Codex's provider for it, which the app needs too).
func CodexAppOut() string {
	c := &codexAppOut
	c.Lock()
	defer c.Unlock()
	if c.pid == 0 {
		return ""
	}
	if time.Since(c.checked) > codexDaemonRecheck {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		ps, err := listProcesses(ctx)
		cancel()
		// what was known stays when the processes can't be listed
		if err == nil {
			c.checked = time.Now()
			if findCodexApp(ps) != c.pid {
				c.user, c.pid, c.tried = "", 0, false
				return ""
			}
		}
	}
	return c.user
}

// CodexAppReopenOnce is whether the Codex app CodexAppOut names may be
// reopened by magpie: true once for each time the account runs out, so
// a reopen that didn't take is never tried again.
func CodexAppReopenOnce() bool {
	c := &codexAppOut
	c.Lock()
	defer c.Unlock()
	if c.pid == 0 || c.tried {
		return false
	}
	c.tried = true
	return true
}

// CodexAppReopenTried is whether magpie tried reopening the Codex app
// CodexAppOut names already, which it doesn't do twice.
func CodexAppReopenTried() bool {
	c := &codexAppOut
	c.Lock()
	defer c.Unlock()
	return c.tried
}
