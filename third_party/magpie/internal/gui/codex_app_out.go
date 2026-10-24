package gui

import (
	"context"
	"log"
	"runtime"
	"time"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/provider"
)

// The Codex app reads its sign-in only as it opens. When the ChatGPT
// account it is on runs out, it greys its Send for every model, magpie's
// too, and making magpie Codex's provider (agent AppOut) reaches it only
// once it is quit and opened again (MasterYi on Discord). On macOS magpie
// does that itself, once each time the account runs out: only while the
// app open then still runs (provider.CodexAppOut, its app-server's pid
// unchanged, so it hasn't read the provider), the account is out, and no
// turn has gone through magpie for codexAppOutIdle. The app's own code
// keeps an unsent message's draft (its composer-prompt-drafts-v2 store)
// and its threads across a quit. Elsewhere, or when the quit didn't take,
// Codex's accounts say to reopen it.

var (
	codexAppOutEvery = 10 * time.Second
	codexAppOutIdle  = 30 * time.Second
	// codexAppOutReopen quits the Codex app at bundle and opens it again
	codexAppOutReopen = agent.ReopenCodexApp
	// codexAppOutBundles are the Codex apps open from before Codex's
	// config last changed
	codexAppOutBundles = func() []string {
		a, err := agent.Find("codex")
		if err != nil {
			return nil
		}
		var out []string
		for _, c := range a.StaleCopies() {
			if c.Kind == "app" && c.Bundle != "" {
				out = append(out, c.Bundle)
			}
		}
		return out
	}
	// codexAgentOut is whether magpie is Codex's provider for its used-up
	// ChatGPT account
	codexAgentOut = func() bool {
		a, err := agent.Find("codex")
		return err == nil && a.AppOut != nil && a.AppOut()
	}
	codexAppOutOS = runtime.GOOS
	// what provider keeps of the app open as the account ran out
	codexAppOutUser   = provider.CodexAppOut
	codexAppOutTried  = provider.CodexAppReopenTried
	codexAppOutOnce   = provider.CodexAppReopenOnce
	codexAppOutUsedUp = provider.CodexUsedUpKnown
)

// codexAppOutJSON is a Codex app left unable to send: the account it is
// signed in to, and whether magpie is still to reopen it.
type codexAppOutJSON struct {
	User      string `json:"user"`
	Reopening bool   `json:"reopening,omitempty"`
}

// codexAppOutState is what Codex's accounts say of it; nil for none.
func codexAppOutState() *codexAppOutJSON {
	u := codexAppOutUser()
	if u == "" || !codexAgentOut() {
		return nil
	}
	return &codexAppOutJSON{User: u, Reopening: codexAppOutOS == "darwin" && !codexAppOutTried()}
}

// reopenCodexAppOut reopens the Codex app left unable to send, if every
// condition above holds now; it says whether it tried.
func reopenCodexAppOut(ctx context.Context, now time.Time) bool {
	if codexAppOutOS != "darwin" || codexAppOutUser() == "" || codexAppOutTried() {
		return false
	}
	if !codexAgentOut() || !codexAppOutUsedUp() {
		return false
	}
	if b := gatewayBusy(); b.Any() || now.Sub(b.Last) < codexAppOutIdle {
		return false
	}
	bundles := codexAppOutBundles()
	if len(bundles) == 0 || !codexAppOutOnce() {
		return false
	}
	for _, b := range bundles {
		c, cancel := context.WithTimeout(ctx, time.Minute)
		if err := codexAppOutReopen(c, b); err != nil {
			log.Printf("codex: reopening %s for its used-up account: %v", b, err)
		}
		cancel()
	}
	return true
}

// keepCodexAppSending runs reopenCodexAppOut every codexAppOutEvery until
// ctx ends.
func keepCodexAppSending(ctx context.Context) {
	t := time.NewTicker(codexAppOutEvery)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case now := <-t.C:
			reopenCodexAppOut(ctx, now)
		}
	}
}
