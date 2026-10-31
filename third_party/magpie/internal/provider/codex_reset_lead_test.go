package provider

import (
	"net/http"
	"os"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// setResetLead saves the user's lead, in minutes, in a sandbox HOME.
func setResetLead(t *testing.T, minutes int) {
	t.Helper()
	s := settings.Load()
	s.CodexResetLead = minutes
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
}

// The spend and routing go by the lead the user set (the owner, 10-30:
// 把提前用卡的时间做成可配置的), the same one, so they can't drift apart
// (#717, #718): three hours set, a reset running out in four is spent in
// one, and routing takes the windows to start again then.
func TestResetLeadFollowsSetting(t *testing.T) {
	signIn(t)
	now := time.Now().Round(0)
	until := now.Add(4 * time.Hour)
	if got := expiringResetSpent(until, now, false, time.Time{}); !got.Equal(until.Add(-30 * time.Minute)) {
		t.Fatalf("unset: spent at %v, want half an hour before", until.Sub(got))
	}
	setResetLead(t, 180)
	want := until.Add(-3 * time.Hour)
	if got := expiringResetSpent(until, now, false, time.Time{}); !got.Equal(want) {
		t.Fatalf("spent %v before it runs out, want 3h", until.Sub(got))
	}
	a := Allowance{{Used: 20, Resets: now.Add(120 * time.Hour), Span: 7 * 24 * time.Hour}}.restartedBy(until)
	if got := a.Restarts(now); !got.Equal(want) {
		t.Fatalf("routing: restarts %v before it runs out, want 3h", until.Sub(got))
	}
	// within the lead already: at once, for both
	now2 := until.Add(-2 * time.Hour)
	if got := expiringResetSpent(until, now2, false, time.Time{}); !got.Equal(now2) {
		t.Fatalf("within the lead: %v", got)
	}
	if got := a.Restarts(now2); !got.Equal(now2) {
		t.Fatalf("routing within the lead: %v", got)
	}
	// and the check spends it then, not before
	e := expiringResets{next: map[string]time.Time{}}
	used := []QuotaWindow{{Span: 7 * 24 * time.Hour, Used: 40}}
	if out, _ := e.check("me@example.com", want.Add(-time.Minute), func() ([]QuotaWindow, *ResetCredits, error) {
		return used, &ResetCredits{Count: 1, Until: &until}, nil
	}, func() (ResetOutcome, error) { return ResetOutcome{Code: "reset"}, nil }); out.Code != "" {
		t.Fatalf("spent a minute before the lead: %+v", out)
	}
	e.next = map[string]time.Time{}
	if out, _ := e.check("me@example.com", want, func() ([]QuotaWindow, *ResetCredits, error) {
		return used, &ResetCredits{Count: 1, Until: &until}, nil
	}, func() (ResetOutcome, error) { return ResetOutcome{Code: "reset"}, nil }); out.Code != "reset" {
		t.Fatalf("not spent at the lead: %+v", out)
	}
}

// A long lead still has several looks inside it: looks tighten to every
// resetExpiryClose half an hour before the lead starts, so a spend that
// fails when it is due is tried again five minutes later, not half an
// hour, and one missed look doesn't push the spend toward the end.
func TestResetLeadLooksInsideLongLead(t *testing.T) {
	signIn(t)
	setResetLead(t, 6*60)
	start := time.Date(2026, 10, 30, 0, 0, 0, 0, time.Local)
	until := start.Add(12 * time.Hour)
	due := until.Add(-6 * time.Hour)
	used := []QuotaWindow{{Span: 7 * 24 * time.Hour, Used: 40}}
	e := expiringResets{next: map[string]time.Time{}}
	var spent time.Time
	var looks []time.Time
	tries := 0
	for now := start; now.Before(until); now = now.Add(resetExpiryClose) {
		e.check("me@example.com", now, func() ([]QuotaWindow, *ResetCredits, error) {
			looks = append(looks, now)
			if !spent.IsZero() {
				return used, nil, nil
			}
			return used, &ResetCredits{Count: 1, Until: &until}, nil
		}, func() (ResetOutcome, error) {
			// the first try fails (ChatGPT not answering)
			if tries++; tries == 1 {
				return ResetOutcome{}, http.ErrHandlerTimeout
			}
			spent = now
			return ResetOutcome{Code: "reset"}, nil
		})
	}
	if want := due.Add(resetExpiryClose); !spent.Equal(want) {
		t.Fatalf("spent %v before it runs out, want %v (looked at %v)", until.Sub(spent), until.Sub(want), looks)
	}
	// looked at every five minutes from half an hour before the lead
	n := 0
	for _, l := range looks {
		if !l.Before(due.Add(-30*time.Minute)) && !l.After(due) {
			n++
		}
	}
	if n != 7 {
		t.Fatalf("%d looks in the half hour before the lead, want 7: %v", n, looks)
	}
	for _, c := range []struct{ lead, near time.Duration }{
		{15 * time.Minute, time.Hour},
		{30 * time.Minute, time.Hour},
		{2 * time.Hour, 150 * time.Minute},
		{24 * time.Hour, 24*time.Hour + 30*time.Minute},
	} {
		if got := resetExpiryNear(c.lead); got != c.near {
			t.Errorf("lead %v: near %v, want %v", c.lead, got, c.near)
		}
	}
}

// A lead out of range, written by hand, or a settings file that can't be
// read, is the default half hour: never none, which would spend a reset
// only as it runs out.
func TestResetLeadUnreadIsDefault(t *testing.T) {
	signIn(t)
	for _, m := range []int{-5, 1, 14, 24*60 + 1, 1 << 30} {
		setResetLead(t, m)
		if got := resetLead(); got != 30*time.Minute {
			t.Errorf("%d minutes: lead %v", m, got)
		}
	}
	setResetLead(t, 15)
	if got := resetLead(); got != 15*time.Minute {
		t.Fatalf("15 minutes: lead %v", got)
	}
	if err := os.WriteFile(settings.Path(), []byte(`{"codexResetLead": 15,`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := resetLead(); got != 30*time.Minute && got != 15*time.Minute {
		// a half-written file is read as the last good one or the default
		t.Fatalf("unreadable: lead %v", got)
	}
	if err := os.WriteFile(settings.Path(), []byte(`{"codexResetLead": "an hour"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := resetLead(); got != 30*time.Minute && got != 15*time.Minute {
		t.Fatalf("not a number: lead %v", got)
	}
}
