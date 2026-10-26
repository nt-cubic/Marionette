package provider

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
)

// A plugin's row set aside with text and nothing counted (AutoClaw's
// 积分余额, Cline's balance, Cursor's Auto + Composer note) is that text:
// QuotaWindow.Text, carried to /api/quotas' span, so the GUI, TUI and CLI
// don't read its used 0 as "100% left". A window that counts, a 0 with a
// limit, and one not set aside (Cursor's On-demand "$0 / $20", a real 0%)
// are readings and stay so.
func TestPluginTextRowIsText(t *testing.T) {
	q := quotaOfPlugin(SubscriptionQuota{Provider: "autoclaw-plugin", User: "u"}, plugin.Usage{Windows: []plugin.UsageWindow{
		{Name: "积分余额", Display: "1,234 积分", Aside: true},
		{Name: "5 hours", Used: 40, Display: "40 / 100"},
		{Name: "Credits", Display: "0 / 50", Limit: 50, Aside: true},
		{Name: "On-demand", Display: "$0 / $20"},
		{Name: "Bare", Aside: true},
	}})
	want := []bool{true, false, false, false, false}
	for i, w := range q.Windows {
		if w.Text != want[i] {
			t.Errorf("%s: Text %v", w.Name, w.Text)
		}
	}
	r := quotaReport([]SubscriptionQuota{q}, nil, nil, time.Now())
	if len(r) != 1 || len(r[0].Windows) != 5 || !r[0].Windows[0].Text || r[0].Windows[1].Text {
		t.Fatalf("report %+v", r)
	}
	b, _ := json.Marshal(r[0].Windows[:2])
	if s := string(b); strings.Count(s, `"text":true`) != 1 {
		t.Fatalf("json %s", s)
	}
}
