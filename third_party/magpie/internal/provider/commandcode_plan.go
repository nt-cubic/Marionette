package provider

// PLUGIN-SERVED (see AGENTS.md): Command Code's plan ("commandcode-plan")
// is a retired built-in subscription, served by its plugin alone,
// @magpie-community/opencode-commandcode-auth (provider.BuiltinRetired).
// Its sign-in, models, requests and usage are the plugin's; what is left
// here reads the accounts signed in to the built-in for the move onto the
// plugin (migrate_side.go), and reads a Command Code key's plan and
// windows for the keyed preset ("commandcode") and the moved account's
// card. A fix belongs in the plugin (github.com/magpie-community/plugins,
// packages/commandcode), with the mover's min raised in
// internal/provider/migrate_side.go.

// A Command Code subscription is a commandcode.ai plan (Go, Pro, GOAT,
// Max, Ultra). The CLI's own account is read, never changed, from
// ~/.commandcode/auth.json; the further accounts the built-in signed in
// kept their keys in logins.json. Which plan a key is on is asked of
// billing/subscriptions, its credits and windows of billing/credits.
//
// Its id is commandcode-plan: "commandcode" is the keyed preset's, which a
// provider added with a key from Studio may already have.

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// CommandCodePlanID is the subscription's id, and its sign-in's.
const CommandCodePlanID = "commandcode-plan"

// CommandCodeMaxOutput is the most max_tokens Command Code takes at
// /alpha/generate, for any model: more is refused ("Too big: expected
// number to be <=200000 at params.max_tokens"). models.dev gives some of
// its models more (DeepSeek V4's 384000, MiniMax M3's 512000).
const CommandCodeMaxOutput = 200_000

// Where Command Code's API and its sign-in page are; vars so tests can
// point them elsewhere.
var (
	cmdAPI    = "https://api.commandcode.ai"
	cmdStudio = "https://commandcode.ai"
)

// cmdAuth is an account's key, as auth.json and the sign-in name it.
type cmdAuth struct {
	APIKey   string `json:"apiKey"`
	UserID   string `json:"userId,omitempty"`
	UserName string `json:"userName,omitempty"`
	KeyName  string `json:"keyName,omitempty"`
}

// ---- the CLI's own account ----------------------------------------------------

func cmdAuthPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".commandcode", "auth.json")
}

// cmdOwn is the account Command Code's CLI is signed in to; ok is false
// when it has none.
func cmdOwn() (who string, a cmdAuth, ok bool) {
	if !readJSON(cmdAuthPath(), &a) || strings.TrimSpace(a.APIKey) == "" {
		return "", cmdAuth{}, false
	}
	return cmdWho(a), a, true
}

// cmdWho names an account: its user name, or its id.
func cmdWho(a cmdAuth) string {
	return firstNonEmpty(a.UserName, a.UserID, "Command Code")
}

// ---- the accounts -------------------------------------------------------------

type cmdLogin struct {
	Login
	auth cmdAuth
}

func cmdSaved(l savedLogin) (cmdAuth, bool) {
	var a cmdAuth
	if json.Unmarshal(l.Auth, &a) != nil || a.APIKey == "" {
		return cmdAuth{}, false
	}
	return a, true
}

// cmdLogins is every Command Code account signed in, the first in use first.
func cmdLogins() []cmdLogin {
	ownUser, own, hasOwn := cmdOwn()
	if !hasOwn {
		ownUser = ""
	}
	var out []cmdLogin
	for _, l := range sideLogins(CommandCodePlanID, ownUser, func(l savedLogin) bool {
		_, ok := cmdSaved(l)
		return ok
	}) {
		a := own
		if !l.saved.own() {
			a, _ = cmdSaved(l.saved)
		}
		out = append(out, cmdLogin{l.Login, a})
	}
	return out
}

// ---- which plan ---------------------------------------------------------------

type cmdSeen struct {
	plan string
	at   time.Time
}

// ---- allowance ----------------------------------------------------------------

// cmdPlans names the plans as the CLI does, by their id's start, longest
// first, with the dollars of credits each gives a month (its getPlanInfo;
// "individual-pro-v1" is the old Pro, $80 of them).
var cmdPlans = []struct {
	id, name string
	monthly  float64
}{
	{"individual-provider", "Provider", 15}, {"individual-pro-v1", "Pro", 80}, {"individual-goat", "GOAT", 70},
	{"individual-ultra", "Ultra", 300}, {"individual-max", "Max", 150}, {"individual-pro", "Pro", 30},
	{"individual-go", "Go", 10}, {"teams-pro", "Teams Pro", 40},
}

func cmdPlanOf(id string) (name string, monthly float64) {
	id = strings.ReplaceAll(strings.ToLower(id), "_", "-")
	for _, p := range cmdPlans {
		if strings.HasPrefix(id, p.id) {
			return p.name, p.monthly
		}
	}
	return "", 0
}

func cmdPlanName(id string) string {
	name, _ := cmdPlanOf(id)
	return name
}

// cmdCredits is /alpha/billing/credits as the CLI's /usage reads it
// (projectUsageView): the dollars left of the month's credits, and of the
// bought and free ones beside them, under "credits"; the plan's windows
// next to it, not in it —
//
//	{"credits":{"planId":"individual-goat-monthly","monthlyCredits":41.2,
//	            "purchasedCredits":5,"freeCredits":0},
//	 "windowLimits":{"limited":true,
//	   "fiveHour":{"used":3.1,"cap":10,"resetAt":1790000000000},
//	   "weekly":{"used":12,"cap":40,"resetAt":1790400000000}},
//	 "sandboxMinutes":{…},"sandboxAccess":false}
//
// resetAt is in milliseconds (the CLI compares it with Date.now()); used
// and cap are only ever divided, one by the other.
type cmdWindow struct {
	Used    any `json:"used"`
	Cap     any `json:"cap"`
	ResetAt any `json:"resetAt"` // unix ms (seconds or a time taken too)
}

type cmdCredits struct {
	Credits struct {
		PlanID    string `json:"planId"`
		Monthly   any    `json:"monthlyCredits"`
		Purchased any    `json:"purchasedCredits"`
		Free      any    `json:"freeCredits"`
	} `json:"credits"`
	Windows *struct {
		FiveHour *cmdWindow `json:"fiveHour"`
		Weekly   *cmdWindow `json:"weekly"`
	} `json:"windowLimits"`
}

// cmdTime reads a reset time: unix seconds or milliseconds, or RFC 3339.
func cmdTime(v any) *time.Time {
	if s, ok := v.(string); ok {
		if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
			return &t
		}
	}
	n, ok := number(v)
	if !ok || n <= 0 {
		return nil
	}
	if n < 1e12 {
		n *= 1000
	}
	t := time.UnixMilli(int64(n))
	return &t
}

// cmdWindows is the plan's 5-hour and weekly windows in a credits reply,
// those it gives a cap.
func cmdWindows(c cmdCredits) []QuotaWindow {
	out := []QuotaWindow{}
	if c.Windows == nil {
		return out
	}
	for _, w := range []struct {
		name string
		span time.Duration
		w    *cmdWindow
	}{{"5 hours", 5 * time.Hour, c.Windows.FiveHour}, {"Weekly", 7 * 24 * time.Hour, c.Windows.Weekly}} {
		if w.w == nil {
			continue
		}
		used, ok1 := number(w.w.Used)
		limit, ok2 := number(w.w.Cap)
		if !ok1 || !ok2 || limit <= 0 {
			continue
		}
		out = append(out, QuotaWindow{Name: w.name, Used: min(100, 100*max(0, used)/limit), Span: w.span,
			ResetsAt: cmdTime(w.w.ResetAt)})
	}
	return out
}

// cmdLeft is the dollars left on the account, the month's credits and the
// bought and free ones together (the CLI's totalRemaining), and the
// month's alone; ok is false when the reply tells none of them.
func cmdLeft(c cmdCredits) (monthly, left float64, ok bool) {
	for i, v := range []any{c.Credits.Monthly, c.Credits.Purchased, c.Credits.Free} {
		if n, is := number(v); is {
			n = max(0, n)
			if i == 0 {
				monthly = n
			}
			left += n
			ok = true
		}
	}
	return monthly, left, ok
}

// cmdQuotaOf makes the credits reply into the plan's windows: the 5-hour
// and weekly limits, and the month's credits as one more. A card's
// Balance is shown instead of its windows — it is a key's money, not an
// allowance — so, set beside them, it hid both limits on the Usage page,
// the panel and the TUI. The dollars are a window when the plan is known,
// used of the CLI's pool (the plan's month of credits, or what is left of
// it if more, and the bought and free ones) as its /usage bar is; a reply
// with neither windows nor a plan is a Balance still.
func cmdQuotaOf(q SubscriptionQuota, c cmdCredits) SubscriptionQuota {
	name, planMonthly := cmdPlanOf(c.Credits.PlanID)
	if name != "" {
		q.Plan = name
	}
	q.Windows = cmdWindows(c)
	monthly, left, known := cmdLeft(c)
	switch {
	case !known:
	case planMonthly > 0:
		pool := max(planMonthly, monthly) + left - monthly
		q.Windows = append(q.Windows, QuotaWindow{Name: "Credits", Used: min(100, 100*(pool-left)/pool),
			Display: money("$", pool-left) + " / " + money("$", pool)})
	case len(q.Windows) == 0:
		q.Balance = money("$", left)
	}
	return q
}

// cmdNoPlan is the plan of an account with no subscription: its key is
// billed against the credits it bought, if any.
const cmdNoPlan = "No plan"

// cmdSubscription is the account's plan (its id, and its name), when its period ends and whether
// it renews then, as billing/subscriptions says; cmdNoPlan when it has
// none. ok is false when that can't be read.
func cmdSubscription(ctx context.Context, a cmdAuth) (id, plan string, until *time.Time, renew string, ok bool) {
	var r struct {
		Success *bool `json:"success"` // false when Command Code couldn't tell ("write CONNECTION_CLOSED …"), though a 200
		Data    *struct {
			PlanID           string `json:"planId"`
			Status           string `json:"status"`
			CurrentPeriodEnd any    `json:"currentPeriodEnd"`
			CancelAtEnd      *bool  `json:"cancelAtPeriodEnd"`
		} `json:"data"`
	}
	if accountJSON(ctx, cmdAPI+"/alpha/billing/subscriptions", a.APIKey, nil, &r) != nil || (r.Success != nil && !*r.Success) {
		return "", "", nil, "", false
	}
	if d := r.Data; d != nil && d.PlanID != "" && d.Status != "canceled" && d.Status != "incomplete_expired" {
		if d.CancelAtEnd != nil {
			renew = map[bool]string{true: "off", false: "auto"}[*d.CancelAtEnd]
		}
		return d.PlanID, firstNonEmpty(cmdPlanName(d.PlanID), d.PlanID), cmdTime(d.CurrentPeriodEnd), renew, true
	}
	return "", cmdNoPlan, nil, "", true
}

// cmdKeyWindows is a Command Code key's plan card (#2451): the plan's
// 5-hour and weekly windows, and its month of credits as one more, as
// the signed-in subscription's card has them. billing/credits leaves the
// plan out, so it is the key's subscription's, read at most every ten
// minutes (billing/subscriptions takes 15–20 seconds at times), and the
// one read before while it can't be read. A key with no windows is a
// pay-as-you-go key: no plan, and no card.
func cmdKeyWindows(ctx context.Context, src planQuotaSource, key string) (string, []QuotaWindow, error) {
	b, err := planBody(ctx, src, key)
	if err != nil {
		return "", nil, err
	}
	var c cmdCredits
	if err := json.Unmarshal(b, &c); err != nil {
		return "", nil, err
	}
	if len(cmdWindows(c)) == 0 {
		return cmdPlanName(c.Credits.PlanID), []QuotaWindow{}, nil
	}
	if c.Credits.PlanID == "" {
		c.Credits.PlanID = cmdKeyPlanID(ctx, key)
	}
	q := cmdQuotaOf(SubscriptionQuota{}, c)
	return q.Plan, q.Windows, nil
}

var cmdKeyPlans = struct {
	sync.Mutex
	m map[string]cmdSeen // plan is the subscription's id here
}{m: map[string]cmdSeen{}}

func cmdKeyPlanID(ctx context.Context, key string) string {
	cmdKeyPlans.Lock()
	seen, had := cmdKeyPlans.m[key]
	cmdKeyPlans.Unlock()
	if had && time.Since(seen.at) < 10*time.Minute {
		return seen.plan
	}
	id, _, _, _, ok := cmdSubscription(ctx, cmdAuth{APIKey: key})
	if !ok {
		return seen.plan
	}
	cmdKeyPlans.Lock()
	cmdKeyPlans.m[key] = cmdSeen{id, time.Now()}
	cmdKeyPlans.Unlock()
	return id
}

// ---- signing in ---------------------------------------------------------------

// cmdKeysURL is Studio's page of API keys, where one is made to paste.
var cmdKeysURL = "https://commandcode.ai/settings/keys"
