package provider

// PLUGIN-SERVED (see AGENTS.md): Cursor ("cursor") is a deprecated built-in
// subscription served by its plugin, @magpie-community/opencode-cursor-auth,
// once moved onto it (provider.Moved; the default for a new sign-in). A
// moved one's sign-ins, models, requests and usage are all the plugin's,
// never this code's (only the move, in migrate*.go, still reads its
// accounts). A fix here alone doesn't reach those users; fix the plugin
// (github.com/magpie-community/plugins, packages/cursor) and raise the
// mover's min in internal/provider/migrate_side.go.

// How much of a Cursor plan's included usage is gone, as the CLI's own
// usage view reads it: the dashboard's current period, split into the
// Cursor Models and Other Models pools.

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"math"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/appdir"
)

// cursorBase is Cursor's API; a var so tests can point it elsewhere.
var cursorBase = "https://api2.cursor.sh"

// cursorKeychain reads cursor-agent's token from the macOS Keychain; a var
// so tests stay off the real one.
var cursorKeychain = runtime.GOOS == "darwin"

// cursorToken is the access token cursor-agent signed in with: in the
// Keychain on a Mac, in its auth.json elsewhere.
func cursorToken() (string, error) {
	if discoveryOff() {
		return "", errAccountDiscoveryOff
	}
	if cursorKeychain {
		if tok := cursorKeychainToken(false); tok != "" {
			return tok, nil
		}
	}
	var auth struct {
		AccessToken string `json:"accessToken"`
	}
	if path := cursorAuthPath(); path != "" && readJSON(path, &auth) && auth.AccessToken != "" {
		return auth.AccessToken, nil
	}
	return "", errorf("cursor-agent is not signed in")
}

// cursorAuthPath is where cursor-agent keeps its sign-in outside the Keychain.
func cursorAuthPath() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	switch runtime.GOOS {
	case "windows":
		dir := appdir.Getenv("APPDATA")
		if dir == "" {
			dir = filepath.Join(home, "AppData", "Roaming")
		}
		return filepath.Join(dir, "Cursor", "auth.json")
	case "darwin":
		return filepath.Join(home, ".cursor", "auth.json")
	}
	dir := appdir.Getenv("XDG_CONFIG_HOME")
	if dir == "" {
		dir = filepath.Join(home, ".config")
	}
	return filepath.Join(dir, "cursor", "auth.json")
}

func cursorSubscriptionUsage(ctx context.Context, plan string) SubscriptionQuota {
	q := SubscriptionQuota{Provider: "cursor", Name: "Cursor", Icon: "cursor", Plan: plan, Windows: []QuotaWindow{}}
	if holding(ctx) {
		q.Error = errNotAsked.Error()
		return q
	}
	tok, err := cursorToken()
	if err == nil {
		q.Windows, err = cursorWindows(ctx, tok)
	}
	if err != nil {
		q.Error = err.Error()
	}
	return q
}

// cursorPeriod is GetCurrentPeriodUsage's reply (Connect JSON).
type cursorPeriod struct {
	BillingCycleEnd  string   `json:"billingCycleEnd"` // epoch millis
	AutoBucketModels []string `json:"autoBucketModels"`
	PlanUsage        *struct {
		Auto  float64 `json:"autoPercentUsed"`
		API   float64 `json:"apiPercentUsed"`
		Total float64 `json:"totalPercentUsed"`
	} `json:"planUsage"`
	SpendLimitUsage *cursorSpend `json:"spendLimitUsage"`
}

// cursorSpend is the period's on-demand spend, in cents. IndividualLimit
// is nil when the user has no limit of their own.
type cursorSpend struct {
	IndividualUsed  json.Number  `json:"individualUsed"`
	IndividualLimit *json.Number `json:"individualLimit"`
	LimitType       string       `json:"limitType"`
	PooledLimit     json.Number  `json:"pooledLimit"` // an int64 string
	PooledUsed      json.Number  `json:"pooledUsed"`
}

// cursorHardLimit is GetHardLimit's reply: the on-demand limit, in dollars.
type cursorHardLimit struct {
	HardLimit           float64 `json:"hardLimit"`
	NoUsageBasedAllowed bool    `json:"noUsageBasedAllowed"`
}

// cursorOnDemand is what Cursor still serves once the included usage is
// spent: kind is "fixed" (up to limit dollars), "unlimited", "disabled" or
// "unavailable" (not known). mine is the member's own spend when used is
// their team's pool's (pool is set).
type cursorOnDemand struct {
	kind              string
	used, limit, mine float64 // dollars
	pool              bool
}

// cursorOnDemandOf derives the On-demand state as cursor-agent 2026.10.01's
// usage view does (usage-data.ts), and as the plugin's onDemandOf: from the
// period's spendLimitUsage and, where that sets no limit of the user's,
// GetHardLimit's reply (nil when it couldn't be read). A team member with
// no limit of their own spends the team's pool, which cursor-agent's view
// calls unlimited but which runs out at pooledLimit for the whole team
// (#1868): that is a fixed limit on the pool's spend.
func cursorOnDemandOf(p cursorPeriod, hard *cursorHardLimit) cursorOnDemand {
	num := func(n json.Number) float64 { f, _ := n.Float64(); return f }
	s := p.SpendLimitUsage
	if s == nil {
		s = &cursorSpend{}
	}
	used := num(s.IndividualUsed) / 100
	var own *cursorOnDemand
	if s.IndividualLimit != nil {
		if l := num(*s.IndividualLimit); l > 0 {
			own = &cursorOnDemand{kind: "fixed", used: used, limit: l / 100}
		} else {
			own = &cursorOnDemand{kind: "disabled", used: used}
		}
	}
	if s.LimitType == "team" {
		switch {
		case own != nil:
			return *own
		case hard != nil && (hard.NoUsageBasedAllowed || hard.HardLimit <= 0):
			return cursorOnDemand{kind: "disabled", used: used}
		case num(s.PooledLimit) > 0:
			return cursorOnDemand{kind: "fixed", used: num(s.PooledUsed) / 100, limit: num(s.PooledLimit) / 100, mine: used, pool: true}
		case hard != nil:
			return cursorOnDemand{kind: "unlimited", used: used}
		}
		return cursorOnDemand{kind: "unavailable", used: used}
	}
	switch {
	case hard == nil && own != nil:
		return *own
	case hard == nil:
		return cursorOnDemand{kind: "unavailable", used: used}
	case hard.NoUsageBasedAllowed:
		return cursorOnDemand{kind: "disabled", used: used}
	case hard.HardLimit >= 2147483647:
		return cursorOnDemand{kind: "unlimited", used: used}
	case hard.HardLimit > 0:
		return cursorOnDemand{kind: "fixed", used: used, limit: hard.HardLimit}
	}
	return cursorOnDemand{kind: "disabled", used: used}
}

// cursorDashboard posts {} to one of Cursor's DashboardService methods and
// decodes its reply into v.
func cursorDashboard(ctx context.Context, token, method string, v any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, cursorBase+"/aiserver.v1.DashboardService/"+method, bytes.NewReader([]byte("{}")))
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Connect-Protocol-Version", "1")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	if res.StatusCode < 200 || res.StatusCode >= 300 {
		return &accountStatusError{status: res.StatusCode}
	}
	return json.Unmarshal(b, v)
}

// cursorWindows: the plan's included usage this billing period. An
// enterprise plan reports spend instead, and gets no windows. An account
// that may spend on-demand once its included usage is gone (a team or
// enterprise that pays for more, #1476) is still served when the pools are
// spent, so they are shown aside, and its On-demand spend is what can run
// out: at its limit, or never when it has none.
func cursorWindows(ctx context.Context, token string) ([]QuotaWindow, error) {
	var data cursorPeriod
	var hard *cursorHardLimit
	done := make(chan struct{})
	go func() {
		defer close(done)
		var h cursorHardLimit
		if cursorDashboard(ctx, token, "GetHardLimit", &h) == nil {
			hard = &h
		}
	}()
	err := cursorDashboard(ctx, token, "GetCurrentPeriodUsage", &data)
	<-done
	if err != nil {
		return []QuotaWindow{}, err
	}
	if data.PlanUsage == nil {
		return []QuotaWindow{}, nil
	}
	var resets *time.Time
	if ms, err := strconv.ParseInt(data.BillingCycleEnd, 10, 64); err == nil && ms > 0 {
		t := time.UnixMilli(ms)
		resets = &t
	}
	u := data.PlanUsage
	inCursorPool := func(model string) bool {
		model = cursorPoolBase(model)
		if model == "auto" {
			model = "default" // the CLI's Auto is default in Cursor's API
		}
		// the server names a family (grok-4.8); the CLI asks for one at an
		// effort or speed (grok-4.8-high-fast)
		return slices.ContainsFunc(data.AutoBucketModels, func(m string) bool { return cursorPoolBase(m) == model }) || cursorFirstPartyModel(model)
	}
	od := cursorOnDemandOf(data, hard)
	pooled := od.kind == "fixed" || od.kind == "unlimited"
	// the two pools fit the line; the total goes in its tooltip
	ws := []QuotaWindow{
		{Name: "Cursor Models", Used: u.Auto, ResetsAt: resets, matches: inCursorPool, Aside: pooled},
		{Name: "Other Models", Used: u.API, ResetsAt: resets, matches: func(model string) bool { return !inCursorPool(model) }, Aside: pooled},
	}
	switch od.kind {
	case "fixed":
		display := cursorDollars(od.used) + " / " + cursorDollars(od.limit)
		if od.pool {
			display += " (you: " + cursorDollars(od.mine) + ")"
		}
		ws = append(ws, QuotaWindow{Name: "On-demand", Used: max(0, min(100, 100*od.used/od.limit)), Display: display, ResetsAt: resets})
	case "unlimited":
		ws = append(ws, QuotaWindow{Name: "On-demand", Display: cursorDollars(od.used), ResetsAt: resets, Aside: true})
	}
	return append(ws, QuotaWindow{Name: "Total", Used: u.Total, ResetsAt: resets, Aside: true}), nil
}

// cursorDollars says a dollar amount to the cent, thousands grouped, as the
// plugin does: a team's pool runs to six figures ($451,200.00).
func cursorDollars(d float64) string {
	s := strconv.FormatFloat(math.Abs(d), 'f', 2, 64)
	whole, cents, _ := strings.Cut(s, ".")
	for i := len(whole) - 3; i > 0; i -= 3 {
		whole = whole[:i] + "," + whole[i:]
	}
	if d < 0 && s != "0.00" {
		return "-$" + whole + "." + cents
	}
	return "$" + whole + "." + cents
}

// cursorPoolBase names a model's family: lower case, without cursor- and
// the effort and speed the CLI adds to it.
func cursorPoolBase(model string) string {
	model = strings.TrimPrefix(strings.ToLower(strings.TrimSpace(model)), "cursor-")
	for {
		b := cursorVariant.ReplaceAllString(model, "")
		if b == model {
			return model
		}
		model = b
	}
}

// Cursor's autoBucketModels can lag model releases: it still omitted Grok
// 4.6/4.7 when the published Cursor Models pool already included them.
// Keep those documented families alongside the server's exact model list.
// See https://cursor.com/docs/models-and-pricing.
func cursorFirstPartyModel(model string) bool {
	model = strings.TrimPrefix(model, "cursor-")
	if model == "default" || model == "composer" || strings.HasPrefix(model, "composer-") {
		return true
	}
	for _, base := range []string{"grok-4.5", "grok-4.6", "grok-4.7"} {
		if model == base || strings.HasPrefix(model, base+"-") {
			return true
		}
	}
	return false
}
