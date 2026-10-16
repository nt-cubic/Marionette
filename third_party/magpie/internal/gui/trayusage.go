package gui

import (
	"cmp"
	"context"
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/fx"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// Settings → Allowances in the menu bar: the windows of the subscriptions and
// plans ticked there, beside the tray icon, for keeping an eye on them
// without opening magpie. The Mac's menu bar draws each as its logo with
// its two windows stacked, "42%" over "18%", side by side (trayimage), or
// as the windows alone, a thin line between cards, when Settings turns the
// logos off; a tray elsewhere gets them as text ("42% · 18% | 10% · 5%"), Windows' in
// the icon's tooltip only, having no room for any.

// trayUsageEvery is how often the text is brought up to date, as Settings
// says (every 3 minutes unless told otherwise). The vendors are asked no
// more than the Usage page asks them: their answers are cached for a
// minute, and this reads the cache.
func trayUsageEvery() time.Duration {
	return time.Duration(settings.Load().TrayUsageEvery) * time.Minute
}

// trayCardID names a card for settings.TrayUsage: its provider, and the
// account when there is one, as two of one vendor can be signed in.
func trayCardID(q provider.SubscriptionQuota) string {
	if q.User == "" {
		return q.Provider
	}
	return q.Provider + "|" + q.User
}

// trayWindows are the windows a card's text shows: those that stop the
// account (not on-demand spending, nor one that counts a single model's
// use), the shortest first where the vendor says how long they run.
func trayWindows(q provider.SubscriptionQuota) []provider.QuotaWindow {
	var ws []provider.QuotaWindow
	for _, w := range q.Windows {
		if !w.Aside && w.Model == "" {
			ws = append(ws, w)
		}
	}
	slices.SortStableFunc(ws, func(a, b provider.QuotaWindow) int {
		if a.Span == 0 || b.Span == 0 {
			return 0
		}
		return cmp.Compare(a.Span, b.Span)
	})
	return ws
}

// trayUsageText is the menu bar's text for a card and the tooltip that
// spells it out: each window's use, or what is left of it (left), and when
// it starts again. With pace (settings.TrayPace) each window's Pace number
// follows its figure, in both, as the Usage page's cards show it.
func trayUsageText(q provider.SubscriptionQuota, now time.Time, left, pace bool) (label, tip string) {
	if strings.HasPrefix(q.Provider, trayTotal) {
		return q.Balance, q.Name // trayTotalCard's: its rows, and its tooltip
	}
	q.Name = trayName(q)
	if q.Error != "" {
		return "", q.Name + ": " + q.Error
	}
	ws := trayWindows(q)
	if len(ws) == 0 {
		if q.Balance == "" {
			return "", ""
		}
		return q.Balance, q.Name + " · " + q.Balance + trayAsOf(q)
	}
	readAt := now
	if q.ReadAt != nil && !q.ReadAt.After(now) {
		readAt = *q.ReadAt
	}
	var short, long []string
	for _, w := range ws {
		used := math.Max(0, math.Min(100, w.Used))
		n := int(math.Round(used))
		word := "used"
		if left {
			used = 100 - used
			n, word = 100-n, "left"
		}
		pct := fmt.Sprintf("%d%%", n)
		row, paceNote := pct, ""
		if p, ok := w.Pace(readAt, now); pace && ok {
			row = pct + " " + trayPace(p, left)
			paceNote = " · pace " + trayPace(p, left)
		}
		short = append(short, row)
		tipPct := pct
		if used != math.Trunc(used) {
			tipPct = fmt.Sprintf("%.1f%%", used)
		}
		line := w.Name + " " + tipPct + " " + word
		if c := w.Count(left); c != "" {
			line = w.Name + " " + c + " · " + tipPct + " " + word
		}
		line += paceNote
		if at := resetAt(w, now); !at.IsZero() && at.After(now) {
			line += " · resets in " + until(at.Sub(now))
		}
		long = append(long, line)
	}
	if len(short) > 2 {
		short = short[:2]
	}
	return strings.Join(short, " · "), q.Name + "\n" + strings.Join(long, "\n") + trayAsOf(q)
}

// trayPace is a window's Pace number as the Usage page writes it
// (paceNumber in app.js): "+12%", "−8%" with a true minus, "±0%". It
// follows the figure beside it, so where what is left is shown (left) its
// sign is turned: +12% left is twelve points more to spare than an even
// pace would have.
func trayPace(p float64, left bool) string {
	if left {
		p = -p
	}
	switch n := int(math.Round(p)); {
	case n > 0:
		return fmt.Sprintf("+%d%%", n)
	case n < 0:
		return fmt.Sprintf("−%d%%", -n)
	default:
		return "±0%"
	}
}

// trayAsOf is the tooltip's word that a card stands in for one that
// couldn't be read just now, and when it was read (the Usage page's
// "As of …"); nothing for a reading just made.
func trayAsOf(q provider.SubscriptionQuota) string {
	if q.AsOf == nil {
		return ""
	}
	return "\nas of " + q.AsOf.Local().Format("Jan 2 15:04") + ", couldn't be read just now"
}

func resetAt(w provider.QuotaWindow, now time.Time) time.Time {
	if w.ResetsAt != nil {
		return *w.ResetsAt
	}
	if w.ResetSecs > 0 {
		return now.Add(time.Duration(w.ResetSecs) * time.Second)
	}
	return time.Time{}
}

// until says a wait in the largest two units: 3d 4h, 2h 10m, 7m.
func until(d time.Duration) string {
	d = d.Round(time.Minute)
	days, hours, mins := int(d/(24*time.Hour)), int(d%(24*time.Hour)/time.Hour), int(d%time.Hour/time.Minute)
	switch {
	case days > 0:
		return fmt.Sprintf("%dd %dh", days, hours)
	case hours > 0:
		return fmt.Sprintf("%dh %dm", hours, mins)
	}
	return fmt.Sprintf("%dm", max(mins, 1))
}

// trayUsageCards are the cards settings.TrayUsages names, in its order,
// among the Usage page's (cached as there), and magpie's own totals it
// names (trayTotal); one gone is left out. The vendors aren't asked when
// only totals are named.
func trayUsageCards(ctx context.Context, ids []string) []provider.SubscriptionQuota {
	if len(ids) == 0 {
		return nil
	}
	var cards []provider.SubscriptionQuota
	if slices.ContainsFunc(ids, func(id string) bool { return !strings.HasPrefix(id, trayTotal) }) {
		cards = slices.Clip(provider.Quotas(ctx)) // appended to below: never into the cache's array
	}
	var currency string
	var rate float64
	var noCost bool
	for _, id := range ids {
		p, ok := trayTotalPeriod(id)
		if !ok {
			continue
		}
		if currency == "" {
			currency, rate = trayCurrency()
			noCost = settings.Load().TrayNoCost
		}
		cards = append(cards, trayTotalCard(id, trayMoney(trayTotalSum(p), noCost), currency, rate))
	}
	return trayPick(cards, ids, time.Now())
}

// trayTotal starts the id of a card of magpie's own count instead of a
// subscription's (#1984): "usage:today", "usage:7d", the tokens every
// agent's requests came to over the Usage page's period, over what they
// cost. A provider's id has no ":", so no subscription's card is taken for
// one.
const trayTotal = "usage:"

// trayTotalPeriod is the period a total's id names: today or the last 7
// days, the Usage page's first two, the ones worth watching from the menu
// bar (the page has no 3-day window to agree with).
func trayTotalPeriod(id string) (usage.Period, bool) {
	p, ok := strings.CutPrefix(id, trayTotal)
	if !ok || usage.Period(p) != usage.Today && usage.Period(p) != usage.Week {
		return "", false
	}
	return usage.Period(p), true
}

// trayTotalSum is a period's sum as the Usage page's Requests tab and the
// panel's Usage tab have it, the pages a click on it opens: the gateway's
// requests and the calls the agents' session files record beside them,
// from the ledger's cached index; a var for tests. The Overview's summary,
// summed before, left those calls out (NextCommit on Discord).
var trayTotalSum = func(p usage.Period) usage.Totals { return usage.QueryPage(p, usage.Filter{}, 0, 1).Sum }

// trayTotalCard is a total as a card: its rows are every token, the
// cache's too, as the Requests tab's and the panel's Tokens count them,
// marked Σ so it doesn't read as the input alone (NextCommit on Discord),
// over the cost, in the currency Settings says, "+" when some calls had no
// known price; nothing beneath the tokens when none had one. Its name is
// the tooltip, which spells out the input, output and cache.
func trayTotalCard(id string, t usage.Totals, currency string, rate float64) provider.SubscriptionQuota {
	when := "Today"
	if p, _ := trayTotalPeriod(id); p == usage.Week {
		when = "Last 7 days"
	}
	rows := []string{"Σ" + trayCount(t.Tokens()+t.CacheRead+t.CacheWrite)}
	tip := when + " in magpie\n" + trayTokens(t)
	if t.Cost > 0 {
		c := usage.FormatCost(t.Cost, currency, rate)
		if t.Unpriced > 0 {
			c += "+"
		}
		rows = append(rows, c)
		tip += "\n≈" + c
	}
	tip += fmt.Sprintf("\n%d requests", t.Calls)
	if t.Unpriced > 0 {
		tip += fmt.Sprintf(", %d with no known price", t.Unpriced)
	}
	return provider.SubscriptionQuota{Provider: id, Name: tip, Balance: strings.Join(rows, " · ")}
}

// trayMoney is a total as the menu bar says it: with its cost, or with no
// cost and no count of calls with no known price when Settings › Menu bar
// cost is off (settings.TrayNoCost, BussanQ on Discord), the tokens alone.
func trayMoney(t usage.Totals, noCost bool) usage.Totals {
	if noCost {
		t.Cost, t.Unpriced = 0, 0
	}
	return t
}

// trayCurrency is the currency Settings shows costs in, and the yuan's
// rate when that is CNY.
func trayCurrency() (currency string, rate float64) {
	currency = settings.Load().Currency
	if currency == "cny" {
		rate = fx.Soon().CNYPerUSD // never waited for, as the Settings page's
	}
	return currency, rate
}

// trayTokens is a total's every token, the cache's too, with what they
// were: "6.3M tokens (912K in · 318K out · 5.1M cached)".
func trayTokens(t usage.Totals) string {
	s := trayCount(t.Tokens()+t.CacheRead+t.CacheWrite) + " tokens (" + trayCount(t.Input) + " in · " + trayCount(t.Output) + " out"
	if cached := t.CacheRead + t.CacheWrite; cached > 0 {
		s += " · " + trayCount(cached) + " cached"
	}
	return s + ")"
}

// trayIdleTip is the tooltip while the menu bar shows no cards: magpie's
// name over what went through it today, as the panel's Usage tab counts it
// (ylorn on Discord: a Linux bar's tooltip said only "magpie"): every
// token, split into input, output and cache as the usage:today card's
// tooltip does (NextCommit on Discord), then the cost and the requests.
func trayIdleTip(t usage.Totals, currency string, rate float64) string {
	if t.Calls == 0 {
		return "magpie\nToday: no requests yet"
	}
	line := "Today: " + trayTokens(t) + "\n"
	if t.Cost > 0 {
		c := usage.FormatCost(t.Cost, currency, rate)
		if t.Unpriced > 0 {
			c += "+"
		}
		line += "≈" + c + " · "
	}
	n := "requests"
	if t.Calls == 1 {
		n = "request"
	}
	return fmt.Sprintf("magpie\n%s%d %s", line, t.Calls, n)
}

// trayCount is a count as the Usage page says it in English: 133M, 1.2M,
// 456K, 8.2K, 950.
func trayCount(n int) string {
	f := float64(n)
	switch {
	case n >= 1e9:
		return fmt.Sprintf("%.2fB", f/1e9)
	case n >= 1e7:
		return fmt.Sprintf("%.0fM", math.Round(f/1e6))
	case n >= 1e6:
		return fmt.Sprintf("%.1fM", f/1e6)
	case n >= 1e5:
		return fmt.Sprintf("%.0fK", math.Round(f/1e3))
	case n >= 1e3:
		return fmt.Sprintf("%.1fK", f/1e3)
	}
	return fmt.Sprint(n)
}

func trayPick(cards []provider.SubscriptionQuota, ids []string, now time.Time) []provider.SubscriptionQuota {
	var out []provider.SubscriptionQuota
	for _, id := range ids {
		if pid, ok := strings.CutSuffix(id, trayInUse); ok {
			if q, ok := trayInUseCard(cards, pid, now); ok {
				out = append(out, q)
			}
			continue
		}
		if i := slices.IndexFunc(cards, func(q provider.SubscriptionQuota) bool { return trayCardID(q) == id }); i >= 0 {
			out = append(out, cards[i])
		}
	}
	return out
}

// trayInUse ends a card id that follows a subscription's account in use
// ("claude|*") rather than naming one: with several accounts, the one the
// gateway is sending to is the one worth watching, whichever it is now.
const trayInUse = "|*"

// trayInUseAccount is the account of an agent's the gateway goes to first;
// a var for tests.
var trayInUseAccount = provider.InUseLogin

// trayServedFresh is how lately an account must have answered through the
// gateway for the menu bar to take it as the one in use; a var for tests.
var trayServedFresh = time.Hour

// trayInUseCard is the card of the provider's account in use, named with
// the account so the tooltip says which it is: the one that answered a
// request through the gateway last, within trayServedFresh — the gateway
// moves on from a spent account, and under Smart picks any, as soon as it
// asks, which the account the agent is signed in to can lag (okingkee on
// X: A used up, B answering, the menu bar still on A) — else the one
// InUseLogin reckons the gateway goes to first; else the one that answered
// last at all (a plugin's accounts, which InUseLogin doesn't know); the
// provider's first card when magpie can't tell (the account in use first
// among them).
func trayInUseCard(cards []provider.SubscriptionQuota, pid string, now time.Time) (provider.SubscriptionQuota, bool) {
	var mine []provider.SubscriptionQuota
	for _, q := range cards {
		if q.Provider == pid {
			mine = append(mine, q)
		}
	}
	if len(mine) == 0 {
		return provider.SubscriptionQuota{}, false
	}
	q := mine[0]
	if len(mine) > 1 {
		last := -1
		for i, c := range mine {
			if c.LastServedAt != nil && (last < 0 || c.LastServedAt.After(*mine[last].LastServedAt)) {
				last = i
			}
		}
		in := -1
		if user := trayInUseAccount(pid); user != "" {
			in = slices.IndexFunc(mine, func(q provider.SubscriptionQuota) bool { return strings.EqualFold(q.User, user) })
		}
		switch {
		case last >= 0 && now.Sub(*mine[last].LastServedAt) < trayServedFresh:
			q = mine[last]
		case in >= 0:
			q = mine[in]
		case last >= 0:
			// a plugin's accounts are no sign-in InUseLogin knows (#1516):
			// the one that answered last, however long ago, over the first
			q = mine[last]
		}
	}
	if q.User != "" && q.Alias == "" {
		q.Name += " · " + q.User // a named account's name is trayName's
	}
	return q, true
}

// trayName is how the tooltip names a card: by the name the user gave its
// account, or the key plan it stands in for, after the card's (#1515).
func trayName(q provider.SubscriptionQuota) string {
	if q.Alias != "" {
		return q.Name + " · " + q.Alias
	}
	return q.Name
}

// trayCell is a card as the Mac's menu bar draws it: its logo, and its
// label's parts stacked, the shortest window over the longer ("42%" over
// "18%"), or its balance alone.
type trayCell struct {
	ID     string // stable provider/account identity, captured with the drawn cell
	Icon   []byte // the logo as the Usage page has it (SVG or PNG); nil for none
	Mono   bool   // a black glyph, drawn in the menu bar's text colour
	Letter string // drawn in its place when there is no logo, or it can't be read
	Plain  bool   // no logo nor letter, as Settings says: the rows alone, a line before
	Rows   []string
}

// trayPlain are the cells without their logos (settings.TrayNoLogos):
// each its rows alone, told from the one before it by a thin line.
func trayPlain(cells []trayCell) []trayCell {
	out := make([]trayCell, len(cells))
	for i, c := range cells {
		out[i] = trayCell{ID: c.ID, Plain: true, Rows: c.Rows}
	}
	return out
}

// trayUsageView is what the tray shows for the cards: a cell for each
// that has a label, with its stable identity for clicks; labels joined by
// " | " for a tray that shows text; and a tooltip spelling each out.
func trayUsageView(cards []provider.SubscriptionQuota, now time.Time, left, pace bool) (cells []trayCell, label, tip string) {
	var labels, tips []string
	for _, q := range cards {
		l, t := trayUsageText(q, now, left, pace)
		if t != "" {
			tips = append(tips, t)
		}
		if l == "" {
			continue
		}
		labels = append(labels, l)
		c := trayCell{ID: trayCardID(q), Rows: strings.Split(l, " · ")}
		for i, r := range c.Rows {
			c.Rows[i] = trayRow(r)
		}
		if strings.HasPrefix(q.Provider, trayTotal) {
			// magpie's own: the bird beside it says whose, no logo of a vendor's
			c.Plain = true
			cells = append(cells, c)
			continue
		}
		c.Icon, c.Mono = trayIconFile(q.Icon)
		if r := []rune(strings.TrimSpace(q.Name)); len(r) > 0 {
			c.Letter = strings.ToUpper(string(r[0]))
		}
		cells = append(cells, c)
	}
	return cells, strings.Join(labels, " | "), strings.Join(tips, "\n\n")
}

// sniTip is the tooltip as a Linux tray item's ToolTip title carries it.
// Omarchy's bar shows the title as plain text and waybar as Pango markup,
// where a bare "&" or "<" loses the whole tooltip; their look-alikes read
// the same in both.
func sniTip(tip string) string {
	return strings.NewReplacer("&", "＆", "<", "‹", ">", "›").Replace(tip)
}

// trayRow is a cell's row cut to fit the menu bar: a balance is a few
// figures ("¥12345.67"), but one read from a plan's own page can be any
// text, and the whole of it is in the tooltip.
func trayRow(r string) string {
	if rs := []rune(r); len(rs) > trayRowMax {
		return strings.TrimSpace(string(rs[:trayRowMax-1])) + "…"
	}
	return r
}

const trayRowMax = 10

// trayIconFile is a card's logo among the page's icons, as its icon() has
// it: a "-color" one and the PNGs in their own colours, the others a black
// glyph (a mask there); nil when there is no such file (a picture the user
// gave their own provider isn't one).
func trayIconFile(name string) (b []byte, mono bool) {
	if name == "" || strings.ContainsAny(name, `/\:`) {
		return nil, false
	}
	if b, err := assets.ReadFile("assets/icons/" + name + ".png"); err == nil {
		return b, false
	}
	b, err := assets.ReadFile("assets/icons/" + name + ".svg")
	if err != nil {
		return nil, false
	}
	return svgArcFlags(b), !strings.HasSuffix(name, "-color")
}

var svgPathData = regexp.MustCompile(`\sd="([^"]*[aA][^"]*)"`)

// svgArcFlags spaces out the flags of a path's arcs ("a4.5 4.5 0 004.5 0",
// the flags run into the number after them), which browsers read and the
// Mac's own SVG drawing doesn't: it draws a line for such an arc, and
// Codex's logo comes out a blot.
func svgArcFlags(svg []byte) []byte {
	return svgPathData.ReplaceAllFunc(svg, func(m []byte) []byte {
		d := string(svgPathData.FindSubmatch(m)[1])
		var out []string
		cmd, arg := byte(0), 0
		for i := 0; i < len(d); {
			c := d[i]
			switch {
			case c == ' ' || c == ',' || c == '\t' || c == '\n' || c == '\r':
				i++
			case c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z':
				if c == 'e' || c == 'E' { // not a command: a number's exponent, never alone
					return m
				}
				cmd, arg = c, 0
				out = append(out, string(c))
				i++
			case (cmd == 'a' || cmd == 'A') && (arg%7 == 3 || arg%7 == 4):
				if c != '0' && c != '1' {
					return m
				}
				out = append(out, string(c))
				arg++
				i++
			default:
				j := svgNumber(d, i)
				if j == i {
					return m
				}
				out = append(out, d[i:j])
				arg++
				i = j
			}
		}
		return []byte(` d="` + strings.Join(out, " ") + `"`)
	})
}

// svgNumber is where the number at d[i:] ends: a sign, digits with at most
// one point, an exponent.
func svgNumber(d string, i int) int {
	j := i
	if j < len(d) && (d[j] == '-' || d[j] == '+') {
		j++
	}
	digits, point := 0, false
	for j < len(d) && (d[j] >= '0' && d[j] <= '9' || d[j] == '.' && !point) {
		if d[j] == '.' {
			point = true
		} else {
			digits++
		}
		j++
	}
	if digits == 0 {
		return i
	}
	if j < len(d) && (d[j] == 'e' || d[j] == 'E') {
		k := j + 1
		if k < len(d) && (d[k] == '-' || d[k] == '+') {
			k++
		}
		if k < len(d) && d[k] >= '0' && d[k] <= '9' {
			for j = k; j < len(d) && d[j] >= '0' && d[j] <= '9'; j++ {
			}
		}
	}
	return j
}

// onTrayUsage brings the tray's text up to date at once, when the Settings
// page changes which card it shows; set by the process that has the tray.
var onTrayUsage func()

// onTrayCellClick receives the stable card ID from the native image.
// Set by the tray process; it must schedule window work without blocking.
var onTrayCellClick func(id string)

// panelQuotaJS safely quotes the card ID for the panel's focus handler.
func panelQuotaJS(card string) string {
	if p, ok := trayTotalPeriod(card); ok {
		return `panelUseFocus("` + string(p) + `")` // the Usage tab, at the total's period
	}
	id, _ := json.Marshal(card) // quoted and escaped: a JSON string is a JS one
	return "panelQuotaFocus(" + string(id) + ")"
}
