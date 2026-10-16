package gateway

import (
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// 0xAncientTwo on X: the Claude subscription was signed out again and again.
// A saved account switched in to Claude Code itself has its directory
// removed; the runs waiting there for their next turn hold its refresh
// token, now Claude Code's, and are let go, so none refreshes it under
// Claude Code. Runs of other accounts, and runs in Claude Code's own home
// (which read its sign-in afresh), wait on.
func TestClaudeHandOverLetsGoOfTheAccountsRuns(t *testing.T) {
	b := newSubscriptionBridge()
	run := func(owner string) *subscriptionRun {
		r := &subscriptionRun{bridge: b, owner: owner, pending: map[string]chan mcpToolResult{}}
		b.mu.Lock()
		b.idle[owner] = r
		r.idleKey = owner
		b.mu.Unlock()
		return r
	}
	moved := run("claude\x00B@example.com")
	other := run("claude\x00a@example.com")
	own := run("claude\x00b@example.com\x00" + ownHome)

	provider.ClaudeHandedOver("b@example.com")

	if !moved.closed {
		t.Fatal("a run in the handed-over account's directory waits on")
	}
	if other.closed || own.closed {
		t.Fatal("another account's run, or Claude Code's own, was let go")
	}
	b.mu.Lock()
	_, kept := b.idle["claude\x00B@example.com"]
	n := len(b.idle)
	b.mu.Unlock()
	if kept || n != 2 {
		t.Fatalf("idle runs after the hand-over: %d, moved kept %v", n, kept)
	}
}
