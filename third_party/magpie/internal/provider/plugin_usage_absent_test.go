package provider

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// A plugin that leaves out used says nothing of how much of a window is
// spent: a 0 is a reading, absent is not. The window goes as its text,
// never "100% left" (QuotaWindow.Text), and has no share in the allowance
// routing ranks on; an account whose windows all say nothing is unknown to
// routing, not the account with the most left (#3650).
func TestPluginWindowWithoutUsedIsNoReading(t *testing.T) {
	now := time.Now()
	q := quotaOfPlugin(SubscriptionQuota{Provider: "loomy-plugin", User: "u"}, plugin.Usage{Windows: []plugin.UsageWindow{
		{Name: "Credits", Display: "0 / 5000"},
		{Name: "Daily", Used: fptr(0), Display: "0 / 100", Limit: 100},
	}})
	if !q.Windows[0].Text || !q.Windows[0].Aside {
		t.Errorf("not said: Text %v, Aside %v", q.Windows[0].Text, q.Windows[0].Aside)
	}
	if q.Windows[1].Text || q.Windows[1].Aside {
		t.Errorf("a 0 that was said: Text %v, Aside %v", q.Windows[1].Text, q.Windows[1].Aside)
	}
	if a := allowanceOf(q.Windows, now); len(a) != 1 || a[0].Name != "Daily" || a[0].Used != 0 {
		t.Errorf("the allowance routing ranks on: %+v", a)
	}
	only := quotaOfPlugin(SubscriptionQuota{Provider: "loomy-plugin", User: "v"}, plugin.Usage{Windows: []plugin.UsageWindow{
		{Name: "Credits", Display: "0 / 5000"},
	}})
	if a := allowanceOf(only.Windows, now); len(a) != 0 {
		t.Errorf("a window that said nothing is an allowance: %+v", a)
	}
}

// A window that said nothing has no share in the allowance routing ranks
// on, through the decode a plugin's reply takes: the window whose used was
// never sent is absent from it, while one that said 0 is there (#3650).
// This one compiles on the base too, so it is the regression that fails
// before the fix.
func TestAWindowWithoutUsedIsNoAllowance(t *testing.T) {
	var u plugin.Usage
	if err := json.Unmarshal([]byte(`{"windows":[{"name":"Credits","display":"0 / 5000"},{"name":"Daily","used":0,"limit":100,"display":"0 / 100"}]}`), &u); err != nil {
		t.Fatal(err)
	}
	q := quotaOfPlugin(SubscriptionQuota{Provider: "loomy-plugin", User: "u"}, u)
	if a := allowanceOf(q.Windows, time.Now()); len(a) != 1 || a[0].Name != "Daily" {
		t.Fatalf("the allowance: %+v", a)
	}
}

// Routing, from a fresh reading and from the one kept on disk alike
// (Allowances, lastAllowances: routedAllowance), leaves an account whose
// windows tell no share out, as not known, rather than ranking it as the
// one with all of it left; one whose window said 0 is known (#3650).
func TestRoutingKnowsNoAccountThatSaidNoShare(t *testing.T) {
	var said, unsaid plugin.Usage
	if err := json.Unmarshal([]byte(`{"windows":[{"name":"Credits","display":"0 / 5000","resetsAt":"2099-01-01T00:00:00Z"}]}`), &unsaid); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal([]byte(`{"windows":[{"name":"Credits","used":0,"display":"0 / 5000","resetsAt":"2099-01-01T00:00:00Z"}]}`), &said); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	if a, ok := routedAllowance("plugin:loomy", "u", quotaOfPlugin(SubscriptionQuota{Provider: "loomy-plugin", User: "u"}, unsaid), now); ok {
		t.Errorf("no share said, yet known: %+v", a)
	}
	a, ok := routedAllowance("plugin:loomy", "v", quotaOfPlugin(SubscriptionQuota{Provider: "loomy-plugin", User: "v"}, said), now)
	if used, _ := a.For("", now); !ok || used != 0 {
		t.Errorf("a used of 0 that was said: known %v, used %v", ok, used)
	}
}

// Through the plugin host: a window the plugin sent without used reaches
// magpie as no share (host.js's usageOf passes it on as null, not 0), so
// the card tells its text and routing doesn't know the account, while a
// window that said used 0 is a reading at 0 (#3650).
func TestPluginHostPassesAnAbsentUsedOn(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	windows := filepath.Join(t.TempDir(), "windows.json")
	if err := os.WriteFile(windows, []byte(`{
		"a@fake": [{"name": "Credits", "display": "0 / 5000"}],
		"full@fake": [{"name": "Credits", "used": 0, "display": "0 / 5000"}]
	}`), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("FAKE_WINDOWS", windows)
	t.Cleanup(plugin.Settle)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	for _, team := range []string{"a", "full"} {
		st, err := StartPluginSignIn("fakeco", 1, map[string]string{"where": "work", "team": team})
		if err != nil {
			t.Fatal(err)
		}
		if err := SubmitSignInCallback(st.ID, "good"); err != nil {
			t.Fatal(err)
		}
	}
	u := LoginUsage(ctx, "plugin:fakeco")
	unsaid, said := u["a@fake"], u["full@fake"]
	if len(unsaid.Windows) != 1 || len(said.Windows) != 1 {
		t.Fatalf("readings: %+v", u)
	}
	if w := unsaid.Windows[0]; !w.Text || !w.Aside || w.Display != "0 / 5000" {
		t.Errorf("not said: %+v", w)
	}
	if w := said.Windows[0]; w.Text || w.Aside {
		t.Errorf("a 0 that was said: %+v", w)
	}
	now := time.Now()
	if a, ok := routedAllowance("plugin:fakeco", "a@fake", unsaid, now); ok {
		t.Errorf("no share said, yet known: %+v", a)
	}
	if _, ok := routedAllowance("plugin:fakeco", "full@fake", said, now); !ok {
		t.Error("a used of 0 that was said is not known")
	}
}

// A window that tells its count (amount of limit) and leaves out used has
// its share in that count: 370 of 1000 is 37% used, read by routing when
// it stops the account and shown with its count when it is aside. Before,
// it was text alone, and an account whose windows all told counts so was
// unknown to routing (plugins#468). A count of 0, what a left-out amount
// reads as, still says nothing.
func TestAWindowsCountIsItsShare(t *testing.T) {
	var u plugin.Usage
	if err := json.Unmarshal([]byte(`{"windows":[
		{"name":"Credits","amount":370,"limit":1000,"unit":"credits","display":"370 / 1000","resetsAt":"2099-01-01T00:00:00Z"},
		{"name":"Bonus","amount":5,"limit":20,"aside":true,"display":"5 / 20"},
		{"name":"Daily","limit":100,"display":"? / 100"}]}`), &u); err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	q := quotaOfPlugin(SubscriptionQuota{Provider: "acme-plugin", User: "u"}, u)
	if w := q.Windows[0]; w.Text || w.Aside || w.Used != 37 {
		t.Errorf("a counted window: Text %v, Aside %v, Used %v", w.Text, w.Aside, w.Used)
	}
	if w := q.Windows[1]; w.Text || !w.Aside || w.Used != 25 {
		t.Errorf("a counted window set aside: Text %v, Aside %v, Used %v", w.Text, w.Aside, w.Used)
	}
	if w := q.Windows[2]; !w.Text || !w.Aside {
		t.Errorf("a limit with no count: Text %v, Aside %v", w.Text, w.Aside)
	}
	a, ok := routedAllowance("plugin:acme", "u", q, now)
	if used, _ := a.For("", now); !ok || used != 37 {
		t.Errorf("routing: known %v, used %v; want the count's 37", ok, used)
	}
}
