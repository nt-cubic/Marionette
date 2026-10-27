package provider

import (
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
)

// cdredfox on X: WorkBuddy's credits come in packages that each end on
// their own day, and the Credits window has no reset. The plugin (0.1.14)
// says when the soonest credits left run out (runsOutAt), and Smart
// routing takes it for the window's renewal, so the account whose credits
// end first is spent first. The window is the one workbuddy 0.1.14 sends
// for the WorkBuddy AI account it was tried on: the Bonus Pack's 232.4
// credits end 10-13 00:39:27 +08.
func TestWorkBuddyCreditsRunOutForRouting(t *testing.T) {
	now := time.Date(2026, 10, 11, 12, 0, 0, 0, time.UTC)
	credits := func(runsOutAt string) plugin.Usage {
		return plugin.Usage{Plan: "Pro", Windows: []plugin.UsageWindow{{Name: "Credits", Used: fptr(100 * 17.6 / 2250),
			Display: "17.6 / 2250", Amount: 17.6, Limit: 2250, Unit: "credits", RunsOutAt: runsOutAt, RunsOut: 232.4}}}
	}
	q := quotaOfPlugin(SubscriptionQuota{}, credits("2026-10-12T16:39:27.000Z"))
	w := q.Windows[0]
	at := time.Date(2026, 10, 12, 16, 39, 27, 0, time.UTC)
	if w.RunsOutAt == nil || !w.RunsOutAt.Equal(at) || w.RunsOut != 232.4 || w.ResetsAt != nil {
		t.Fatalf("window: %+v", w)
	}
	if r := allowanceOf(q.Windows, now).Renewal("hy3", now); len(r) != 1 || !r[0].Equal(at) {
		t.Fatalf("renewal: %v, want %v", r, at)
	}
	// an older plugin, or a list it couldn't read: not known, as before
	q = quotaOfPlugin(SubscriptionQuota{}, credits(""))
	if q.Windows[0].RunsOutAt != nil {
		t.Fatalf("no runsOutAt: %+v", q.Windows[0])
	}
	if r := allowanceOf(q.Windows, now).Renewal("hy3", now); len(r) != 1 || !r[0].IsZero() {
		t.Fatalf("renewal not known: %v", r)
	}
	// already past: not known, the window isn't taken as renewing then
	q = quotaOfPlugin(SubscriptionQuota{}, credits("2026-10-11T11:00:00Z"))
	if r := allowanceOf(q.Windows, now).Renewal("hy3", now); len(r) != 1 || !r[0].IsZero() {
		t.Fatalf("passed: %v", r)
	}
	// a window that resets says so, and runs out later: its reset stands;
	// sooner, what is left is lost then, and that is its renewal
	reset := now.Add(48 * time.Hour)
	a := Allowance{{Used: 10, Resets: reset, RunsOut: now.Add(72 * time.Hour)}}
	if r := a.Renewal("m", now); !r[0].Equal(reset) {
		t.Fatalf("later run-out moved the reset: %v", r)
	}
	a[0].RunsOut = now.Add(24 * time.Hour)
	if r := a.Renewal("m", now); !r[0].Equal(now.Add(24 * time.Hour)) {
		t.Fatalf("sooner run-out: %v", r)
	}
	// the share used and the rest are the window's, as before: For,
	// Full and the rest don't read RunsOut
	if used, _ := a.For("m", now); used != 10 {
		t.Fatalf("used: %v", used)
	}
}
