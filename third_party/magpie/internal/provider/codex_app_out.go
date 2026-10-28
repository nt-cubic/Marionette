package provider

import (
	"context"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// The Codex app greys its Send for every model, magpie's included, while
// the ChatGPT account it is on is out (MasterYi on Discord), till magpie is
// its provider and its app-server restarted (codex_app_server.go). What
// follows tells the account is out as soon as a reply says so, not at the
// next look five minutes on.

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
