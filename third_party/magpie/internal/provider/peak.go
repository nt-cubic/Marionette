package provider

import (
	"cmp"
	"slices"
	"strings"

	"github.com/yetone/magpie/internal/catalog"
)

// peakBill is a vendor that bills DeepSeek's models by the hour a call is
// made at: its list price is the off-peak one, doubled in the peak hours
// (catalog.Peak). Each one's own pricing page says so, and so does its
// models.dev entry's cost multiplier, which models.dev's api.json leaves
// out. Others that resell DeepSeek's models keep one price at every hour:
// OpenCode Zen lists deepseek-v4.1-flash at $0.30/$1.20 flat
// (https://opencode.ai/docs/zen); a relay's price is its own.
type peakBill struct {
	// at is whether a base URL is the vendor's API
	at func(base string) bool
	// ids are its preset's id and its subscription's: how a provider with
	// no URL of its own (a plugin's, one removed since) is told
	ids  []string
	peak catalog.Peak
	// models are the ids it bills so, without a maker's prefix
	// ("deepseek/"); nil: every model it serves
	models []string
}

var peakBills = []peakBill{
	// DeepSeek's own API, every model: "Off-peak rates are half of the
	// peak rates" (https://api-docs.deepseek.com/quick_start/pricing)
	{at: func(b string) bool { return HostOf(b) == "api.deepseek.com" }, ids: []string{"deepseek"}, peak: catalog.PeakDeepSeek},
	// Command Code's DeepSeek V4 models, its Provider API's and its plan's
	// (https://commandcode.ai/docs/resources/pricing-limits): its
	// deepseek-v4-flash-fast is one price at every hour
	{at: func(b string) bool { return HostOf(b) == "api.commandcode.ai" }, ids: []string{"commandcode", CommandCodePlanID}, peak: catalog.PeakWeekdays,
		models: []string{"deepseek-v4.1-flash", "deepseek-v4.1-flash-fast", "deepseek-v4-flash", "deepseek-v4-flash-vision-exp", "deepseek-v4-pro"}},
	// OpenCode Go's (https://opencode.ai/docs/go): Off-Peak and Peak rows
	{at: func(b string) bool { return openCodeGateway(b) == "go" }, ids: []string{"opencode-go"}, peak: catalog.PeakWeekdays,
		models: []string{"deepseek-v4.1-flash", "deepseek-v4-pro"}},
}

// PeakOf is the hours a call to model at p is billed double in, by the
// vendor p sends it to: its base URLs say which, and only a provider with
// none is told by its preset or id. A plugin's base (plugin://) names no
// vendor, so the Command Code plan its plugin serves is told by its id
// (treeman on Discord: its DeepSeek models showed no peak hours). Empty
// for any other provider or model.
func (p Provider) PeakOf(model string) catalog.Peak {
	urls := slices.DeleteFunc([]string{p.Chat, p.Responses, p.Anthropic}, func(u string) bool {
		return strings.TrimSpace(u) == "" || strings.HasPrefix(u, pluginBase)
	})
	m := strings.ToLower(strings.TrimSpace(model))
	m = m[strings.LastIndexByte(m, '/')+1:]
	for _, b := range peakBills {
		var is bool
		if len(urls) > 0 {
			is = slices.ContainsFunc(urls, b.at)
		} else {
			is = slices.Contains(b.ids, cmp.Or(p.Preset, p.ID))
		}
		if is && (b.models == nil || slices.Contains(b.models, m)) {
			return b.peak
		}
	}
	return ""
}
