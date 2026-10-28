package provider

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// pluginWindowsCard is a card as quotaOfPlugin makes it from a plugin's usage,
// the windows given as the plugin's own JSON.
func pluginWindowsCard(t *testing.T, provider, user, windows string) SubscriptionQuota {
	t.Helper()
	var u plugin.Usage
	if err := json.Unmarshal([]byte(`{"windows":`+windows+`}`), &u); err != nil {
		t.Fatal(err)
	}
	return quotaOfPlugin(SubscriptionQuota{Provider: provider, User: user}, u)
}

// TestCreditWindowShapes: a window is counted by the day by its shape,
// whoever sends it: a count of used against granted in credits or money,
// from amount of limit or a display "x / y" with what vendors write about
// it, and never a share alone, tokens, requests, ACUs or a balance (#3321).
// Each window is as its plugin (magpie-community/plugins) makes it.
func TestCreditWindowShapes(t *testing.T) {
	cases := []struct {
		who, window string
		ok          bool
		used, total float64
		unit        string
	}{
		// workbuddy: display and amount of limit
		{"WorkBuddy", `{"name":"Credits","used":8.94,"display":"500.52 / 5600","amount":500.52,"limit":5600,"unit":"credits"}`, true, 500.52, 5600, "credits"},
		// trae (CN): amount of limit, no display
		{"Trae CN", `{"name":"Credits","used":71.43,"amount":4678.52,"limit":6550,"unit":"credits"}`, true, 4678.52, 6550, "credits"},
		// qoder: display alone, its unit in it
		{"Qoder", `{"name":"Credits","used":41.15,"display":"1234.5 / 3000 credits"}`, true, 1234.5, 3000, "credits"},
		{"Qoder, a big pool", `{"name":"Shared credits","used":75,"display":"1.5e+06 / 2e+06 credits"}`, true, 1.5e6, 2e6, "credits"},
		// opendesign: display alone
		{"Open Design", `{"name":"Week","used":12,"span":604800,"display":"120 / 1000 credits"}`, true, 120, 1000, "credits"},
		// mimo: display alone, compact
		{"MiMo", `{"name":"Token Plan","used":12.3,"display":"12.3M / 100M credits"}`, true, 12.3e6, 100e6, "credits"},
		// commandcode: dollars in the display, named Credits
		{"Command Code", `{"name":"Credits","used":24.68,"display":"$12.34 / $50.00"}`, true, 12.34, 50, "usd"},
		// codearts: compact display with a note, amount of limit exact
		{"CodeArts", `{"name":"总积分","used":40,"display":"1.2万 / 3万 · 300 即将过期","amount":12000,"limit":30000,"unit":"credits"}`, true, 12000, 30000, "credits"},
		{"a display with a note alone", `{"name":"Credits","used":24.68,"display":"1,234 / 5,000 · 300 即将过期"}`, true, 1234, 5000, "credits"},
		// joycode: 积分, and the rest after a ·
		{"JoyCode", `{"name":"通用积分","used":12,"amount":120,"limit":1000,"unit":"积分","display":"120 / 1,000 · 剩余 880"}`, true, 120, 1000, "credits"},
		// kiro, minimax, raccoon: amount of limit in credits
		{"Kiro", `{"name":"Credits","used":10,"display":"5.5 / 55","amount":5.5,"limit":55,"unit":"credits","span":2592000}`, true, 5.5, 55, "credits"},
		{"MiniMax", `{"name":"Credits","used":20,"display":"200 / 1000","amount":200,"limit":1000,"unit":"credits"}`, true, 200, 1000, "credits"},
		// cline: amount of limit, no unit, named Credits
		{"Cline", `{"name":"Credits","used":25,"limit":40,"amount":10,"display":"10 / 40"}`, true, 10, 40, "credits"},
		// trae (global): dollars by its unit
		{"Trae", `{"name":"Dollar Usage","used":30,"amount":6,"limit":20,"unit":"usd"}`, true, 6, 20, "usd"},
		// not credits
		{"Devin's ACUs", `{"name":"ACUs","used":5,"display":"12.5 / 250 ACUs"}`, false, 0, 0, ""},
		{"a share alone", `{"name":"Cursor Models","used":40}`, false, 0, 0, ""},
		{"CodeArts' free tokens", `{"name":"免费额度","used":10,"display":"10万 / 100万","amount":100000,"limit":1000000,"unit":"tokens","aside":true}`, false, 0, 0, ""},
		{"ZCode's prompts", `{"name":"5 hours","used":10,"amount":20,"limit":200,"display":"20 / 200","span":18000}`, false, 0, 0, ""},
		{"AutoClaw's balance", `{"name":"积分余额","used":0,"aside":true,"display":"1200"}`, false, 0, 0, ""},
		{"Cline's credits used", `{"name":"Credits","used":0,"aside":true,"display":"12 credits used"}`, false, 0, 0, ""},
		{"no figures", `{"name":"Credits","used":100,"display":"no credits left"}`, false, 0, 0, ""},
		{"nothing granted", `{"name":"Credits","used":0,"display":"0 / 0"}`, false, 0, 0, ""},
	}
	testenv.SetHome(t, t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	now := time.Now()
	check := func(who string, q SubscriptionQuota, ok bool, used, total float64, unit string) {
		t.Helper()
		noteDailyCredits([]SubscriptionQuota{q}, now)
		b, _ := os.ReadFile(creditsPath())
		accts, _ := parseCredits(b)
		a := accts[q.Provider+"|u|"+q.Windows[0].Name]
		if (a != nil) != ok {
			t.Errorf("%s: counted %v, want %v", who, a != nil, ok)
			return
		}
		if !ok {
			return
		}
		if a.Last.Used != used || a.Last.Total != total {
			t.Errorf("%s: read %v / %v, want %v / %v", who, a.Last.Used, a.Last.Total, used, total)
		}
		if unit == "credits" {
			unit = ""
		}
		if d := withDailyCredits([]SubscriptionQuota{q}, now)[0].Daily; d == nil || d.Unit != unit || d.Window != q.Windows[0].Name {
			t.Errorf("%s: the card's days %+v, want in %q", who, d, unit)
		}
	}
	for i, c := range cases {
		check(c.who, pluginWindowsCard(t, fmt.Sprintf("p%d", i), "u", "["+c.window+"]"), c.ok, c.used, c.total, c.unit)
	}
	// copilot's built-in premium requests: a count, of requests
	check("Copilot", SubscriptionQuota{Provider: "copilot", User: "u", Windows: []QuotaWindow{{Name: "Premium requests", Used: 40, Display: "120 / 300"}}}, false, 0, 0, "")
}

// TestDailyCreditsEveryVendor: any card with a credits window has its
// days, a plugin's without a display too (Trae CN, #3321); each pool of a
// card is counted on its own, a breakdown set aside and the shorter spans
// of one allowance aren't, and a total started again counts nothing.
func TestDailyCreditsEveryVendor(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	day := func(d, h int) time.Time { return time.Date(2026, 10, d, h, 0, 0, 0, time.Local) }
	trae := func(used float64) SubscriptionQuota {
		return pluginWindowsCard(t, "trae-cn", "Moxuan", `[{"name":"Credits","used":10,"amount":`+jsonNum(used)+`,"limit":6550,"unit":"credits"}]`)
	}
	qoder := func(main, addon float64) SubscriptionQuota {
		return pluginWindowsCard(t, "qoder", "me@x.com", `[{"name":"Credits","used":10,"display":"`+jsonNum(main)+` / 3000 credits"},{"name":"Add-on credits","used":10,"display":"`+jsonNum(addon)+` / 1000 credits"}]`)
	}
	codearts := func(total, basic float64) SubscriptionQuota {
		return pluginWindowsCard(t, "codearts", "hw", `[{"name":"总积分","used":10,"amount":`+jsonNum(total)+`,"limit":30000,"unit":"credits","display":"x"},{"name":"基础积分","used":10,"amount":`+jsonNum(basic)+`,"limit":10000,"unit":"credits","aside":true}]`)
	}
	design := func(hours, week float64) SubscriptionQuota {
		return pluginWindowsCard(t, "opendesign", "d", `[{"name":"5 hours","used":1,"span":18000,"display":"`+jsonNum(hours)+` / 100 credits"},{"name":"Week","used":1,"span":604800,"display":"`+jsonNum(week)+` / 1000 credits"}]`)
	}
	read := func(at time.Time, qs ...SubscriptionQuota) { noteDailyCredits(qs, at) }
	read(day(1, 9), trae(4600), qoder(100, 0), codearts(1000, 500), design(10, 200))
	read(day(1, 12), trae(4678.52), qoder(130, 20), codearts(1300, 700), design(30, 220))
	// a new month: Trae's total started again, CodeArts' too
	read(day(2, 1), trae(5), qoder(130, 20), codearts(0, 0), design(0, 220))
	read(day(2, 9), trae(40), qoder(160, 20), codearts(50, 50), design(15, 235))
	// an error and a reading kept from before count nothing
	errd := trae(6000)
	errd.Error = "Trae: 401"
	kept := trae(6000)
	asOf := day(2, 9)
	kept.AsOf = &asOf
	read(day(2, 10), errd, kept)

	got := withDailyCredits([]SubscriptionQuota{trae(40), qoder(160, 20), codearts(50, 50), design(15, 235)}, day(2, 12))
	want := map[string][]*DailyCredits{
		"Moxuan":   {{Since: "2026-10-01", Days: []DayCredits{{"2026-10-01", 78.52}, {"2026-10-02", 35}}, Window: "Credits"}},
		"me@x.com": {{Since: "2026-10-01", Days: []DayCredits{{"2026-10-01", 30}, {"2026-10-02", 30}}, Window: "Credits"}, {Since: "2026-10-01", Days: []DayCredits{{"2026-10-01", 20}}, Window: "Add-on credits"}},
		"hw":       {{Since: "2026-10-01", Days: []DayCredits{{"2026-10-01", 300}, {"2026-10-02", 50}}, Window: "总积分"}},
		"d":        {{Since: "2026-10-01", Days: []DayCredits{{"2026-10-01", 20}, {"2026-10-02", 15}}, Window: "Week"}},
	}
	for _, q := range got {
		all := append([]*DailyCredits{q.Daily}, q.MoreDaily...)
		if !reflect.DeepEqual(all, want[q.User]) {
			b, _ := json.Marshal(all)
			w, _ := json.Marshal(want[q.User])
			t.Errorf("%s's days: %s, want %s", q.Provider, b, w)
		}
	}
	// money's days say their currency
	cc := pluginWindowsCard(t, "commandcode", "c", `[{"name":"Credits","used":1,"display":"$1.00 / $50.00"}]`)
	read(day(3, 9), cc)
	cc2 := pluginWindowsCard(t, "commandcode", "c", `[{"name":"Credits","used":1,"display":"$3.50 / $50.00"}]`)
	read(day(3, 10), cc2)
	if d := withDailyCredits([]SubscriptionQuota{cc2}, day(3, 11))[0].Daily; d == nil || d.Unit != "usd" || len(d.Days) != 1 || d.Days[0].Used != 2.5 {
		t.Errorf("Command Code's days: %+v", d)
	}
}

func jsonNum(f float64) string { b, _ := json.Marshal(f); return string(b) }

// TestDailyCreditsKeptBefore: WorkBuddy's days, kept by provider and
// account alone before #3321, are still its card's, and counting goes on
// from its last reading under the window's own name.
func TestDailyCreditsKeptBefore(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	day := func(d, h int) time.Time { return time.Date(2026, 10, d, h, 0, 0, 0, time.Local) }
	old := `{
  "workbuddy|李雷": {
    "since": "2026-09-20",
    "last": {"used": 130.25, "total": 5600, "at": "2026-10-01T23:00:00+08:00"},
    "days": {"2026-09-20": 12.5, "2026-10-01": 30.25}
  }
}`
	if err := os.MkdirAll(filepath.Dir(creditsPath()), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(creditsPath(), []byte(old), 0o600); err != nil {
		t.Fatal(err)
	}
	card := wbCard("workbuddy", "李雷", 130.25, 5600)
	want := &DailyCredits{Since: "2026-09-20", Days: []DayCredits{{"2026-09-20", 12.5}, {"2026-10-01", 30.25}}, Window: "Credits"}
	if d := withDailyCredits([]SubscriptionQuota{card}, day(2, 9))[0].Daily; !reflect.DeepEqual(d, want) {
		t.Errorf("before a new reading: %+v, want %+v", d, want)
	}
	noteDailyCredits([]SubscriptionQuota{wbCard("workbuddy", "李雷", 140.25, 5600)}, day(2, 10))
	want.Days = append(want.Days, DayCredits{"2026-10-02", 10})
	if d := withDailyCredits([]SubscriptionQuota{card}, day(2, 11))[0].Daily; !reflect.DeepEqual(d, want) {
		t.Errorf("after: %+v, want %+v", d, want)
	}
	b, _ := os.ReadFile(creditsPath())
	var kept map[string]json.RawMessage
	if err := json.Unmarshal(b, &kept); err != nil || len(kept) != 1 || kept["workbuddy|李雷|Credits"] == nil {
		t.Errorf("the file: %s", b)
	}
}
