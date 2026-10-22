package gui

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/agent"
	"github.com/yetone/magpie/internal/edit"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/sessions"
	"github.com/yetone/magpie/internal/usage"
	"github.com/yetone/magpie/internal/usageapi"
)

// The Usage page's JSON is built in usageapi, which the gateway's
// /v1/magpie/usage answers from too.
type (
	usageGroup = usageapi.Group
	usageJSON  = usageapi.Usage
	ledgerRow  = usageapi.Row
	ledgerJSON = usageapi.Ledger
)

var (
	usageState   = usageapi.Page
	ledgerPage   = usageapi.LedgerPage
	periodOf     = usageapi.PeriodOf
	ledgerFilter = usageapi.FilterOf
)

func init() {
	usageapi.Clients = func() []usageapi.Client {
		var out []usageapi.Client
		for _, a := range agent.Clients() {
			out = append(out, usageapi.Client{ID: a.ID, Name: a.Name, Icon: a.Icon})
		}
		return out
	}
}

// csvStamp names the selected day, or, when no day is selected, the period
// and the day of now, the moment its rows were read at.
func csvStamp(p usage.Period, day string, now time.Time) string {
	if _, err := time.Parse(time.DateOnly, day); err == nil {
		return "magpie-requests-day-" + day
	}
	if p.IsRange() { // the days picked, which already say when
		return "magpie-requests-" + strings.Replace(string(p), "..", "-to-", 1)
	}
	// a rolling window ("last:24h0m0s") without the colon, which Windows
	// doesn't take in a file name (#2075)
	return "magpie-requests-" + strings.ReplaceAll(string(p), ":", "-") + "-" + now.Format(time.DateOnly)
}

// contentJSON is what was said in a request, or why it can't be told.
type contentJSON struct {
	Found bool `json:"found"`
	// Why not: "session" (the request named none), "agent" (magpie reads the session
	// files of Claude Code, Claude Desktop and Codex only), "missing" (the files have no
	// such call: deleted, moved, or not written yet) or "read" (the file wouldn't read)
	Why   string `json:"why,omitempty"`
	Model string `json:"model,omitempty"`
	sessions.Content
}

// requestContent finds the call a request is in its agent's session files and reads it.
func requestContent(agent, session string, from, to, at time.Time) contentJSON {
	out := contentJSON{Content: sessions.Content{Input: []sessions.Part{}, Output: []sessions.Part{}}}
	switch {
	case session == "":
		out.Why = "session"
	case agent != "claude" && agent != "claude-desktop" && agent != "codex":
		out.Why = "agent"
	default:
		c, ok := sessions.FindCall(session, from, to, at)
		if !ok {
			out.Why = "missing"
			break
		}
		content, err := sessions.ContentOf(c)
		if err != nil {
			out.Why = "read"
			break
		}
		out.Found, out.Model, out.Content = true, c.Model, content
	}
	return out
}

func usageRoutes(mux *http.ServeMux, w Windows) {
	mux.HandleFunc("GET /api/usage", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, usageState(periodOf(r.URL.Query().Get("period"))))
	})
	// the ledger: the period's calls, newest first, a page at a time
	mux.HandleFunc("GET /api/usage/requests", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		offset, _ := strconv.Atoi(q.Get("offset"))
		limit, _ := strconv.Atoi(q.Get("limit"))
		writeJSON(rw, ledgerPage(periodOf(q.Get("period")), ledgerFilter(q), offset, limit))
	})
	// the heatmap (#1369): the last 53 weeks a day each, of the requests the
	// page's filters keep, read from the same index as the page
	mux.HandleFunc("GET /api/usage/heatmap", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, usage.HeatmapOf(ledgerFilter(r.URL.Query())))
	})
	// what was said in one request, read from the agent's session file when the
	// row is opened, between two times (the call's own, or a gateway request's
	// span): magpie keeps no copy
	mux.HandleFunc("GET /api/usage/requests/content", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		from, err1 := time.Parse(time.RFC3339Nano, q.Get("from"))
		to, err2 := time.Parse(time.RFC3339Nano, q.Get("to"))
		if err1 != nil || err2 != nil {
			http.Error(rw, "from and to are times", http.StatusBadRequest)
			return
		}
		// at: when the call should have ended (a gateway request's start and
		// its time); none is the window's end
		at, err := time.Parse(time.RFC3339Nano, q.Get("at"))
		if err != nil {
			at = to
		}
		writeJSON(rw, requestContent(q.Get("agent"), q.Get("session"), from, to, at))
	})
	// the same CSV to a browser (magpie web), which saves it itself
	mux.HandleFunc("GET /api/usage/requests.csv", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		p := periodOf(q.Get("period"))
		// one moment for the rows and the day in the file's name
		now := usage.Clock()
		rows := usage.LedgerOfAt(p, ledgerFilter(q), now).Rows
		rw.Header().Set("Content-Type", "text/csv; charset=utf-8")
		rw.Header().Set("Content-Disposition", `attachment; filename="`+csvStamp(p, q.Get("day"), now)+`.csv"`)
		usage.WriteCSV(rw, rows)
	})
	// the rows the ledger shows, all its pages, as a CSV in Downloads
	mux.HandleFunc("POST /api/usage/requests/export", func(rw http.ResponseWriter, r *http.Request) {
		q := r.URL.Query()
		p := periodOf(q.Get("period"))
		now := usage.Clock()
		rows := usage.LedgerOfAt(p, ledgerFilter(q), now).Rows
		var b bytes.Buffer
		if err := usage.WriteCSV(&b, rows); err != nil {
			fail(rw, err)
			return
		}
		dir := downloads()
		stamp := csvStamp(p, q.Get("day"), now)
		name := filepath.Join(dir, stamp+".csv")
		for i := 2; ; i++ { // never over an earlier one
			if _, err := os.Stat(name); err != nil {
				break
			}
			name = filepath.Join(dir, fmt.Sprintf("%s-%d.csv", stamp, i))
		}
		if err := edit.WriteAtomic(name, b.Bytes()); err != nil {
			fail(rw, err)
			return
		}
		_ = w.OpenFolder(dir) // saved either way; the path is in the answer
		writeJSON(rw, map[string]any{"path": tilde(name), "rows": len(rows)})
	})
	// The subscriptions' quotas, the plans' bought with a key, and the
	// keys' balances come from the
	// vendors, which can be slow or unreachable, so the page asks for them
	// apart from the local log.
	// ?asked=1 is the user opening or refreshing the page: Claude Code's
	// own /usage is run at once (provider.AskClaudeUsage).
	// Only then are they read while the user has allowances read only
	// when asked (#1518).
	mux.HandleFunc("GET /api/usage/quotas", func(rw http.ResponseWriter, r *http.Request) {
		ctx := r.Context()
		if r.URL.Query().Get("asked") != "" {
			provider.AskClaudeUsage()
			ctx = provider.Asked(ctx)
		}
		ctx, cancel := context.WithTimeout(ctx, 12*time.Second)
		defer cancel()
		// a WorkBuddy (China) account's card says how its daily check-in
		// went (#694)
		qs := provider.InProviderOrder(provider.WithCheckins(provider.Quotas(ctx)))
		// and what each window holds whole, by what magpie routed in it
		qs = usage.WithWindowHolds(qs, usage.Clock())
		// and when each began, for its Pace number (fflake33)
		qs = provider.WithWindowStarts(qs, usage.Clock())
		// an account's card may be the stale copy a read under way will
		// replace: the page asks again until it has landed (#959)
		if provider.SubscriptionUsageReading() {
			rw.Header().Set("X-Magpie-Reading", "1")
		}
		writeJSON(rw, qs)
	})
	// One card read again, from its refresh button (#840): ?provider= and,
	// of a card with several accounts, &user=; the others are left as they
	// were read. The answer is every card, as GET's.
	mux.HandleFunc("POST /api/usage/quotas/refresh", func(rw http.ResponseWriter, r *http.Request) {
		id := r.URL.Query().Get("provider")
		if id == "" {
			http.Error(rw, "provider is required", http.StatusBadRequest)
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 30*time.Second)
		defer cancel()
		provider.RefreshUsage(ctx, id, r.URL.Query().Get("user"))
		writeJSON(rw, provider.WithWindowStarts(usage.WithWindowHolds(provider.InProviderOrder(provider.WithCheckins(provider.Quotas(ctx))), usage.Clock()), usage.Clock()))
	})
	// WorkBuddy's daily check-in pressed now, from the Usage card, for
	// each account not in yet today, as `magpie accounts checkin` does; the
	// card is read again after
	mux.HandleFunc("POST /api/usage/workbuddy-checkin", func(rw http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
		defer cancel()
		rs := provider.CheckInWorkBuddy(ctx)
		if rs == nil {
			rs = []provider.WorkBuddyCheckin{}
		}
		writeJSON(rw, rs)
	})
	// and Trae CN's, for each Trae CN account (#694)
	mux.HandleFunc("POST /api/usage/trae-checkin", func(rw http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
		defer cancel()
		rs := provider.CheckInTrae(ctx)
		if rs == nil {
			rs = []provider.WorkBuddyCheckin{}
		}
		writeJSON(rw, rs)
	})
	// and MiniMax Code's, for each MiniMax Code (China) account (#811)
	mux.HandleFunc("POST /api/usage/minimax-checkin", func(rw http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
		defer cancel()
		rs := provider.CheckInMiniMax(ctx)
		if rs == nil {
			rs = []provider.WorkBuddyCheckin{}
		}
		writeJSON(rw, rs)
	})
	// and Qoder's daily credits, for each Qoder and Qoder CN account
	mux.HandleFunc("POST /api/usage/qoder-checkin", func(rw http.ResponseWriter, r *http.Request) {
		ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
		defer cancel()
		rs := provider.CheckInQoder(ctx)
		if rs == nil {
			rs = []provider.WorkBuddyCheckin{}
		}
		writeJSON(rw, rs)
	})
	// and a plugin's own, for each account of the provider it names
	mux.HandleFunc("POST /api/usage/plugin-checkin", func(rw http.ResponseWriter, r *http.Request) {
		var in struct{ Provider string }
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil || in.Provider == "" {
			fail(rw, errors.New("no provider"))
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), time.Minute)
		defer cancel()
		rs := provider.CheckInPlugins(ctx, in.Provider)
		if rs == nil {
			rs = []provider.WorkBuddyCheckin{}
		}
		writeJSON(rw, rs)
	})
	// what was left of each window over time, for the quota cards' curves
	// (#651); ?days= back
	mux.HandleFunc("GET /api/usage/quotas/history", func(rw http.ResponseWriter, r *http.Request) {
		days := r.URL.Query().Get("days")
		hs := provider.QuotaHistories(provider.QuotaHistorySince(days, time.Now()), "", "")
		// and a remote magpie's, for its cards (office/codex)
		ctx, cancel := context.WithTimeout(r.Context(), 10*time.Second)
		defer cancel()
		writeJSON(rw, append(hs, provider.RemoteQuotaHistories(ctx, days)...))
	})
	// Historical whole-window estimates come only from this computer's
	// readings and ledger. Selecting a cycle never asks a vendor.
	mux.HandleFunc("GET /api/usage/quotas/history/holds", func(rw http.ResponseWriter, r *http.Request) {
		args := r.URL.Query()
		from, e1 := time.Parse(time.RFC3339Nano, args.Get("from"))
		to, e2 := time.Parse(time.RFC3339Nano, args.Get("to"))
		now := usage.Clock()
		if e1 != nil || e2 != nil || !from.Before(to) || to.After(now) || args.Get("provider") == "" || args.Get("user") == "" {
			http.Error(rw, "provider, user and a completed time range are required", http.StatusBadRequest)
			return
		}
		hist := provider.QuotaHistories(now.Add(-45*24*time.Hour), args.Get("provider"), args.Get("user"))
		writeJSON(rw, usage.HistoricalWindowHolds(provider.CachedCards(now), hist, from, to))
	})
	// spends one of a Codex account's rate-limit resets, which the page
	// has asked the user about first; what it did comes back
	mux.HandleFunc("POST /api/usage/codex-reset", func(rw http.ResponseWriter, r *http.Request) {
		var in struct{ User string }
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		ctx, cancel := context.WithTimeout(r.Context(), 20*time.Second)
		defer cancel()
		out, err := provider.UseCodexReset(ctx, in.User)
		if err != nil {
			fail(rw, err)
			return
		}
		if out.Code == "reset" {
			// routing and the Providers page know the windows started
			// again by the time the page hears so, not at a later
			// request's reading (#1491)
			who := in.User
			if who == "" {
				who, _ = provider.CodexSignedIn()
			}
			wait, done := context.WithTimeout(ctx, 10*time.Second)
			provider.AwaitAllowance(wait, "codex", who)
			done()
		}
		writeJSON(rw, out)
	})
}
