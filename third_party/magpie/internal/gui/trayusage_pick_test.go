package gui

import (
	"slices"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// A subscription's card shows the windows Settings picked for it
// (settings.TrayWindows, StrikingRaw on X: Claude's 5 hours alone, not two
// small numbers squeezed together), named by how long they run, so a
// vendor's order doesn't matter; the tooltip still spells out every one,
// one picked that is gone shows every window rather than none, and a card
// of another provider is left as it was.
func TestTrayWindowsPicked(t *testing.T) {
	now := time.Date(2026, 10, 30, 12, 0, 0, 0, time.UTC)
	claude := provider.SubscriptionQuota{Provider: "claude", Name: "Claude", User: "a@b.c", Windows: []provider.QuotaWindow{
		{Name: "7 days", Used: 70, Span: 7 * 24 * time.Hour}, // the vendor's order, longest first
		{Name: "5 hours", Used: 42, Span: 5 * time.Hour},
		{Name: "Opus", Used: 90, Span: 7 * 24 * time.Hour, Model: "opus"},
	}}
	codex := provider.SubscriptionQuota{Provider: "codex", Name: "Codex", User: "x@y.z", Windows: []provider.QuotaWindow{
		{Name: "5 hours", Used: 10, Span: 5 * time.Hour},
		{Name: "Weekly", Used: 20, Span: 7 * 24 * time.Hour},
	}}
	cards := []provider.SubscriptionQuota{claude, codex}
	rows := func(pick map[string][]string) ([][]string, string) {
		cells, _, tip := trayCardsView(cards, now, false, false, pick)
		var out [][]string
		for _, c := range cells {
			out = append(out, c.Rows)
		}
		return out, tip
	}
	same := func(got, want [][]string) bool { return slices.EqualFunc(got, want, slices.Equal) }

	all, allTip := rows(nil)
	if !same(all, [][]string{{"42%", "70%"}, {"10%", "20%"}}) {
		t.Fatalf("none picked: %q", all)
	}
	got, tip := rows(map[string][]string{"claude": {"5h"}})
	if !same(got, [][]string{{"42%"}, {"10%", "20%"}}) {
		t.Fatalf("5h alone: %q", got)
	}
	if tip != allTip {
		t.Errorf("the tooltip lost a window:\n%s\nwant\n%s", tip, allTip)
	}
	if got, _ = rows(map[string][]string{"claude": {"7d"}, "codex": {"7d"}}); !same(got, [][]string{{"70%"}, {"20%"}}) {
		t.Fatalf("7d alone: %q", got)
	}
	// a picked window gone (the vendor dropped it): every window, not an
	// empty card
	if got, _ = rows(map[string][]string{"claude": {"month"}}); !same(got, all) {
		t.Fatalf("gone: %q", got)
	}
	// the Linux tray's text follows it too
	if _, label, _ := trayCardsView(cards, now, true, false, map[string][]string{"claude": {"5h"}}); label != "58% | 90% · 80%" {
		t.Fatalf("text: %q", label)
	}
}

// Windows are named by how long they run; one whose span isn't known by
// the vendor's name for it.
func TestTrayWindowKind(t *testing.T) {
	for _, c := range []struct {
		w    provider.QuotaWindow
		want string
	}{
		{provider.QuotaWindow{Span: 5 * time.Hour}, "5h"},
		{provider.QuotaWindow{Span: 5*time.Hour + 3*time.Second}, "5h"},
		{provider.QuotaWindow{Span: 24 * time.Hour}, "1d"},
		{provider.QuotaWindow{Span: 7 * 24 * time.Hour}, "7d"},
		{provider.QuotaWindow{Span: 30 * 24 * time.Hour}, "month"},
		{provider.QuotaWindow{Span: 31 * 24 * time.Hour}, "month"},
		{provider.QuotaWindow{Span: 15 * time.Minute}, "15m"},
		{provider.QuotaWindow{Name: " Credits "}, "name:credits"},
	} {
		if got := trayWindowKind(c.w); got != c.want {
			t.Errorf("%+v: %q, want %q", c.w, got, c.want)
		}
	}
}

// Settings is told each window the menu bar can show by its kind, and only
// those: not one a single model's use counts, nor money aside; the cards
// the cache keeps are left as they were.
func TestWithTrayKinds(t *testing.T) {
	qs := []provider.SubscriptionQuota{{Provider: "claude", Windows: []provider.QuotaWindow{
		{Name: "5 hours", Span: 5 * time.Hour},
		{Name: "Opus", Span: 7 * 24 * time.Hour, Model: "opus"},
		{Name: "Extra", Span: 30 * 24 * time.Hour, Aside: true},
	}}, {Provider: "deepseek", Balance: "¥5"}}
	out := withTrayKinds(qs)
	if ws := out[0].Windows; ws[0].TrayKind != "5h" || ws[1].TrayKind != "" || ws[2].TrayKind != "" {
		t.Fatalf("kinds: %+v", ws)
	}
	if qs[0].Windows[0].TrayKind != "" {
		t.Fatal("the cache's window was written")
	}
	if out[1].Balance != "¥5" {
		t.Fatalf("balance card: %+v", out[1])
	}
}
