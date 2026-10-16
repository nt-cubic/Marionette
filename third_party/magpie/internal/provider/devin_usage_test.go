package provider

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/testenv"
)

// TestDevinUsage: magpie's built-in Devin account showed no usage while the
// Devin plugin showed its day, week and ACU windows; both read
// GetUserStatus now, and a key Devin refuses says to sign in again. The
// CLI's own account had no plan either, where the plugin's said its tier.
func TestDevinUsage(t *testing.T) {
	home := claudeHome(t)
	data := filepath.Join(home, "data")
	t.Setenv("XDG_DATA_HOME", data)
	os.MkdirAll(filepath.Join(data, "devin"), 0o700)
	os.WriteFile(filepath.Join(data, "devin", "credentials.toml"), devinCredentials("devin-session-token$good", "", "", ""), 0o600)
	exe := filepath.Join(home, "devin")
	testenv.Program(t, exe, `#!/bin/sh
printf 'Logged in (via Devin).\n\nUser:\n  Email:             dev@example.com\n\nAccount:\n  Tier:              Devin Pro\n'
`)
	fakeDevin(t, exe)
	forgetDevinStatus()
	old := firstAsk
	firstAsk = time.Minute // a loaded machine's shell takes longer than a look's first wait
	t.Cleanup(func() { firstAsk = old })

	var asked map[string]any
	fake := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/exa.seat_management_pb.SeatManagementService/GetUserStatus" || r.Header.Get("Connect-Protocol-Version") != "1" {
			http.NotFound(w, r)
			return
		}
		b, _ := io.ReadAll(r.Body)
		json.Unmarshal(b, &asked)
		if strings.Contains(string(b), "refused") {
			w.WriteHeader(http.StatusUnauthorized)
			io.WriteString(w, `{"code":"unauthenticated","message":"bad key"}`)
			return
		}
		io.WriteString(w, `{"userStatus":{"planStatus":{"planInfo":{"planName":"Pro"},"planEnd":"2026-11-01T00:00:00Z",
			"dailyQuotaRemainingPercent":75,"dailyQuotaResetAtUnix":"1790000000",
			"weeklyQuotaRemainingPercent":"40","weeklyQuotaResetAtUnix":"1790500000",
			"acuLimit":"250","acuConsumed":12.345,"overageBalanceMicros":"5000000"}}}`)
	}))
	defer fake.Close()
	t.Setenv("WINDSURF_API_SERVER_URL", fake.URL)

	ls, ok := builtinLogins("devin")
	// the CLI's own account is named by its tier, as the plugin names it
	if !ok || len(ls) != 1 || ls[0].User != "dev@example.com" || ls[0].Plan != "Devin Pro" {
		t.Fatalf("logins %+v %v", ls, ok)
	}
	q := loginQuota(context.Background(), ls[0])
	if q.Error != "" {
		t.Fatal(q.Error)
	}
	if md, _ := asked["metadata"].(map[string]any); md["apiKey"] != "devin-session-token$good" || md["ideName"] != "devin-cli" {
		t.Fatalf("asked %v", asked)
	}
	if len(q.Windows) != 3 {
		t.Fatalf("windows %+v", q.Windows)
	}
	day, week, acu := q.Windows[0], q.Windows[1], q.Windows[2]
	if day.Name != "1 day" || day.Used != 25 || day.ResetsAt == nil || day.ResetsAt.Unix() != 1790000000 {
		t.Fatalf("day %+v", day)
	}
	if week.Name != "7 days" || week.Used != 60 || week.Span != 7*24*time.Hour {
		t.Fatalf("week %+v", week)
	}
	if acu.Name != "ACUs" || !acu.Aside || acu.Display != "12.35 / 250 ACUs" || acu.ResetsAt == nil {
		t.Fatalf("acu %+v", acu)
	}
	if q.Until == nil || q.Until.Format("2006-01-02") != "2026-11-01" || q.Balance != "$5.00" {
		t.Fatalf("plan %+v", q)
	}

	os.WriteFile(filepath.Join(data, "devin", "credentials.toml"), devinCredentials("devin-session-token$refused", "", "", ""), 0o600)
	if q := loginQuota(context.Background(), ls[0]); q.Error != "Devin's sign-in has expired — sign in again" {
		t.Fatalf("refused %+v", q)
	}
}

// TestDevinWeekUsedUpHolds: 面条 on magpie's Discord — Devin has a daily
// and a weekly quota, and with the week used up only the day showed, at
// 0%, while routing kept sending to the account. JSON leaves a 0 out, so
// the week came as no weeklyQuotaRemainingPercent beside the day's 100
// (the rest as the owner's real Teams account answers GetUserStatus).
func TestDevinWeekUsedUpHolds(t *testing.T) {
	const status = `{"planInfo":{"teamsTier":"TEAMS_TIER_DEVIN_TEAMS_V2","planName":"Teams","monthlyPromptCredits":-1,"isTeams":true,"isDevin":true,"billingStrategy":"BILLING_STRATEGY_QUOTA"},
		"planStart":"2026-09-26T21:45:55Z","planEnd":"2026-10-26T21:45:55Z","availablePromptCredits":-1,
		"dailyQuotaRemainingPercent":100,"overageBalanceMicros":"46807746",
		"dailyQuotaResetAtUnix":"1791705600","weeklyQuotaResetAtUnix":"1792051200"}`
	var st devinPlanStatus
	if err := json.Unmarshal([]byte(status), &st); err != nil {
		t.Fatal(err)
	}
	var q SubscriptionQuota
	devinQuota(&q, st)
	if len(q.Windows) != 2 || q.Windows[0].Name != "1 day" || q.Windows[0].Used != 0 ||
		q.Windows[1].Name != "7 days" || q.Windows[1].Used != 100 || q.Windows[1].ResetsAt == nil || q.Windows[1].ResetsAt.Unix() != 1792051200 {
		t.Fatalf("windows %+v", q.Windows)
	}
	now := time.Unix(1791700000, 0)
	if till := allowanceOf(q.Windows, now).Full("swe-2", 100, now); till.Unix() != 1792051200 {
		t.Fatalf("held till %v, want the week's reset", till)
	}

	// billed otherwise, a share left out is no quota
	var credits devinPlanStatus
	json.Unmarshal([]byte(strings.Replace(status, "BILLING_STRATEGY_QUOTA", "BILLING_STRATEGY_CREDITS", 1)), &credits)
	var c SubscriptionQuota
	devinQuota(&c, credits)
	if len(c.Windows) != 1 || c.Windows[0].Name != "1 day" {
		t.Fatalf("credits windows %+v", c.Windows)
	}
}
