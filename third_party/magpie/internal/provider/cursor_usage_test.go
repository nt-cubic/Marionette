package provider

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// Cursor's current period comes back as its two pools and the total, each
// resetting when the billing cycle ends.
func TestCursorWindows(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/aiserver.v1.DashboardService/GetCurrentPeriodUsage" || r.Header.Get("Authorization") != "Bearer tok" {
			http.Error(rw, "no", http.StatusUnauthorized)
			return
		}
		_ = json.NewEncoder(rw).Encode(map[string]any{
			"billingCycleEnd": "1792833042000",
			"planUsage":       map[string]any{"autoPercentUsed": 12.5, "apiPercentUsed": 40, "totalPercentUsed": 20},
		})
	}))
	defer srv.Close()
	old := cursorBase
	cursorBase = srv.URL
	defer func() { cursorBase = old }()

	ws, err := cursorWindows(context.Background(), "tok")
	if err != nil {
		t.Fatal(err)
	}
	if len(ws) != 3 || ws[0].Used != 12.5 || ws[1].Used != 40 || ws[2].Used != 20 {
		t.Fatalf("windows = %+v", ws)
	}
	if ws[0].Name != "Cursor Models" || ws[1].Name != "Other Models" || ws[2].Name != "Total" {
		t.Fatalf("pool names = %+v", ws)
	}
	if ws[0].ResetsAt == nil || ws[0].ResetsAt.UnixMilli() != 1792833042000 {
		t.Fatalf("resets = %v", ws[0].ResetsAt)
	}
	if _, err := cursorWindows(context.Background(), "bad"); err == nil {
		t.Fatal("a refused token is an error")
	}
}

func TestCursorAllowanceUsesModelPool(t *testing.T) {
	now := time.Date(2026, 9, 28, 0, 0, 0, 0, time.UTC)
	resets := time.UnixMilli(1792833042000)
	for _, tc := range []struct {
		name                 string
		cursor, other, total float64
		models               []string
	}{
		{"other pool full", 25, 100, 100, []string{"default", "composer-2.5", "cursor-grok-4.5-high", "future-first-party", "Grok-4.8"}},
		{"cursor pool full", 100, 25, 100, nil},
		{"only aggregate full", 25, 40, 100, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				_ = json.NewEncoder(w).Encode(map[string]any{
					"billingCycleEnd":  "1792833042000",
					"planUsage":        map[string]any{"autoPercentUsed": tc.cursor, "apiPercentUsed": tc.other, "totalPercentUsed": tc.total},
					"autoBucketModels": tc.models,
				})
			}))
			defer srv.Close()
			old := cursorBase
			cursorBase = srv.URL
			t.Cleanup(func() { cursorBase = old })
			ws, err := cursorWindows(t.Context(), "tok")
			if err != nil {
				t.Fatal(err)
			}
			a := allowanceOf(ws, now)
			firstParty := []string{"grok-4.7-xhigh-fast", "cursor-grok-4.7-high-fast", "cursor-grok-4.6-high-fast", "grok-4.5-fast-high", "auto", "default", "composer-2.5", "COMPOSER-2.5-FAST", "composer"}
			if tc.models != nil {
				firstParty = append(firstParty, "future-first-party", "grok-4.8-high", "cursor-grok-4.8-xhigh-fast")
			}
			for _, pool := range []struct {
				models []string
				used   float64
			}{
				{firstParty, tc.cursor},
				{[]string{"claude-opus-5-5", "gpt-5.6-sol", "gemini-3.1-pro", "grok-3", "grok-4.70", "grok-4.80-high", "unknown-model"}, tc.other},
			} {
				for _, model := range pool.models {
					if used, renews := a.For(model, now); used != pool.used || len(renews) != 1 || !renews[0].Equal(resets) {
						t.Errorf("%s: used %g, renews %v; want %g and one pool reset", model, used, renews, pool.used)
					}
					until := a.Full(model, 98, now)
					if pool.used < 98 && !until.IsZero() || pool.used >= 98 && !until.Equal(resets) {
						t.Errorf("%s: full until %v with %g%% used", model, until, pool.used)
					}
					if used, _ := a.For(model, resets); used != 0 || !a.Full(model, 98, resets).IsZero() {
						t.Errorf("%s: quota did not renew at its reset", model)
					}
				}
			}
		})
	}
}

// #1476 (and fottencity on Discord): a team or enterprise that pays for
// on-demand usage once the included usage is gone. Cursor goes on serving
// it, so the spent pools must not read as the account used up; its
// on-demand spend is what runs out. The replies are GetCurrentPeriodUsage's
// and GetHardLimit's as cursor-agent 2026.10.01 decodes them (Connect JSON
// of GetCurrentPeriodUsageResponse.SpendLimitUsage: cents, pooledLimit an
// int64 string; GetHardLimitResponse: dollars), the plugin's fixtures.
func TestCursorOnDemandKeepsTheAccount(t *testing.T) {
	now := time.Date(2026, 9, 28, 0, 0, 0, 0, time.UTC)
	resets := time.UnixMilli(1792833042000)
	run := func(t *testing.T, spend map[string]any, hard any) []QuotaWindow {
		t.Helper()
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Header.Get("Authorization") != "Bearer tok" {
				http.Error(w, "no", http.StatusUnauthorized)
				return
			}
			switch r.URL.Path {
			case "/aiserver.v1.DashboardService/GetHardLimit":
				if code, ok := hard.(int); ok {
					http.Error(w, "no", code)
					return
				}
				_ = json.NewEncoder(w).Encode(hard)
			case "/aiserver.v1.DashboardService/GetCurrentPeriodUsage":
				p := map[string]any{"billingCycleEnd": "1792833042000", "planUsage": map[string]any{"autoPercentUsed": 100, "apiPercentUsed": 100, "totalPercentUsed": 100}}
				if spend != nil {
					p["spendLimitUsage"] = spend
				}
				_ = json.NewEncoder(w).Encode(p)
			default:
				http.NotFound(w, r)
			}
		}))
		t.Cleanup(srv.Close)
		old := cursorBase
		cursorBase = srv.URL
		t.Cleanup(func() { cursorBase = old })
		ws, err := cursorWindows(t.Context(), "tok")
		if err != nil {
			t.Fatal(err)
		}
		return ws
	}
	type win struct {
		Name    string
		Used    float64
		Display string
		Aside   bool
	}
	shape := func(ws []QuotaWindow) []win {
		var out []win
		for _, w := range ws {
			out = append(out, win{w.Name, w.Used, w.Display, w.Aside})
		}
		return out
	}
	usedUp := func(ws []QuotaWindow) bool {
		a := allowanceOf(ws, now)
		return !a.Full("claude-opus-5-5", 98, now).IsZero() || !a.Full("composer-2.5", 98, now).IsZero()
	}

	t.Run("a team member's own limit is the allowance", func(t *testing.T) {
		ws := run(t, map[string]any{"totalSpend": 52000, "individualLimit": 50000, "individualUsed": 12345, "individualRemaining": 37655, "limitType": "team", "pooledLimit": "0"}, map[string]any{})
		want := []win{{"Cursor Models", 100, "", true}, {"Other Models", 100, "", true}, {"On-demand", 24.69, "$123.45 / $500.00", false}, {"Total", 100, "", true}}
		if got := shape(ws); fmt.Sprint(got) != fmt.Sprint(want) {
			t.Fatalf("windows = %+v\nwant %+v", got, want)
		}
		if usedUp(ws) {
			t.Fatal("a team member with on-demand left is counted used up")
		}
		if ws[2].ResetsAt == nil || !ws[2].ResetsAt.Equal(resets) {
			t.Fatalf("on-demand resets %v", ws[2].ResetsAt)
		}
	})
	t.Run("a team with a hard limit, no pool and none of the member's never runs out", func(t *testing.T) {
		ws := run(t, map[string]any{"individualUsed": 900, "limitType": "team"}, map[string]any{"hardLimit": 2000, "perUserMonthlyLimitDollars": 0})
		for _, w := range ws {
			if !w.Aside {
				t.Errorf("%s not aside: %+v", w.Name, shape(ws))
			}
		}
		if len(ws) != 4 || ws[2].Name != "On-demand" || ws[2].Display != "$9.00" || usedUp(ws) {
			t.Errorf("windows = %+v", shape(ws))
		}
	})
	// #1868 (PaSSw0rds, a real Team account whose on-demand is turned on for
	// the whole organization, in #1476): spendLimitUsage has no
	// individualLimit at all, the pool's limit and remainder are int64
	// strings and its spend a number, and GetHardLimit answers the pool's
	// limit in dollars with or without a team id. The pool is what runs
	// out, for the whole team.
	t.Run("a team's org-wide on-demand pool is the allowance", func(t *testing.T) {
		pool := func(used int, remaining string) map[string]any {
			return map[string]any{"totalSpend": 6334270, "pooledLimit": "45120000", "pooledUsed": used, "pooledRemaining": remaining, "individualUsed": 19855, "limitType": "team"}
		}
		for i, hard := range []any{map[string]any{"hardLimit": 451200}, map[string]any{"hardLimit": 451200, "isDynamicTeamLimit": true}, 500} {
			ws := run(t, pool(6334270, "38785730"), hard)
			want := []win{{"Cursor Models", 100, "", true}, {"Other Models", 100, "", true}, {"On-demand", 100 * 6334270.0 / 45120000, "$63,342.70 / $451,200.00 (you: $198.55)", false}, {"Total", 100, "", true}}
			if got := shape(ws); fmt.Sprint(got) != fmt.Sprint(want) || usedUp(ws) {
				t.Errorf("%d: windows = %+v, used up %v\nwant %+v", i, got, usedUp(ws), want)
			}
		}
		// the pool spent: the account is used up, though the member spent little of it
		ws := run(t, pool(45120000, "0"), map[string]any{"hardLimit": 451200})
		if got := shape(ws)[2]; got != (win{"On-demand", 100, "$451,200.00 / $451,200.00 (you: $198.55)", false}) || !usedUp(ws) {
			t.Errorf("spent: %+v, used up %v", shape(ws), usedUp(ws))
		}
		// nothing spent from the pool yet: Connect JSON leaves pooledUsed out
		ws = run(t, map[string]any{"individualUsed": 900, "limitType": "team", "pooledLimit": "100000"}, 500)
		if got := shape(ws)[2]; got != (win{"On-demand", 0, "$0.00 / $1,000.00 (you: $9.00)", false}) || usedUp(ws) {
			t.Errorf("none spent: %+v, used up %v", shape(ws), usedUp(ws))
		}
	})
	t.Run("a personal hard limit is the on-demand limit; the top int32 is none", func(t *testing.T) {
		ws := run(t, map[string]any{"individualUsed": 500, "limitType": "user"}, map[string]any{"hardLimit": 20})
		if got := shape(ws)[2]; got != (win{"On-demand", 25, "$5.00 / $20.00", false}) || usedUp(ws) {
			t.Fatalf("windows = %+v", shape(ws))
		}
		if ws := run(t, map[string]any{"individualUsed": 500, "limitType": "user"}, map[string]any{"hardLimit": 2147483647}); usedUp(ws) || !ws[2].Aside {
			t.Fatalf("unlimited: %+v", shape(ws))
		}
		// spent to its limit: the account is used up again
		ws = run(t, map[string]any{"individualUsed": 2500, "limitType": "user"}, map[string]any{"hardLimit": 20})
		if got := shape(ws)[2]; got != (win{"On-demand", 100, "$25.00 / $20.00", false}) || !usedUp(ws) {
			t.Fatalf("spent: %+v", shape(ws))
		}
	})
	t.Run("no on-demand allowed, none set or not known: the pools are the allowance", func(t *testing.T) {
		want := []win{{"Cursor Models", 100, "", false}, {"Other Models", 100, "", false}, {"Total", 100, "", true}}
		for i, c := range []struct {
			spend map[string]any
			hard  any
		}{
			{map[string]any{"individualUsed": 0, "limitType": "user"}, map[string]any{}},
			{map[string]any{"individualUsed": 0, "limitType": "user"}, map[string]any{"hardLimit": 50, "noUsageBasedAllowed": true}},
			{map[string]any{"individualLimit": 0, "individualUsed": 0, "limitType": "team"}, map[string]any{"hardLimit": 2000}},
			{map[string]any{"individualUsed": 0, "limitType": "team"}, map[string]any{"hardLimit": 2000, "noUsageBasedAllowed": true}},
			{map[string]any{"individualUsed": 0, "limitType": "team"}, 403},
			{nil, 500},
		} {
			ws := run(t, c.spend, c.hard)
			if got := shape(ws); fmt.Sprint(got) != fmt.Sprint(want) || !usedUp(ws) {
				t.Errorf("%d: windows = %+v, used up %v", i, got, usedUp(ws))
			}
		}
	})
}
