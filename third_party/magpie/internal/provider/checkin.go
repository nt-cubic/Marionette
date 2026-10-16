package provider

// A daily check-in, whichever vendor's: WorkBuddy's (workbuddy_checkin.go)
// and Trae CN's (trae_checkin.go). Each account in use is checked in once
// a Beijing day, except Qoder's campaigns, rechecked every 30 minutes.
// What came of it is kept in the vendor's file by its key for reuse
// (see workbuddy_checkin.go for when a failure is tried again).

import (
	"context"
	"encoding/json"
	"log"
	"sort"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// checkinAcct is an account checked in each day.
type checkinAcct struct {
	User     string
	On       bool
	key      string // its entry in the file; "" for one that can't be told apart
	provider string // the provider its card is under, which the user can switch off
	do       func(context.Context) WorkBuddyCheckin
}

// checkiner checks the accounts it lists in, keeping what came of it at
// path; mu keeps two check-ins (the loop and a press) from asking for one
// account at once.
type checkiner struct {
	path     string
	now      func() time.Time
	mu       *sync.Mutex
	by       string // the vendor, WorkBuddyCheckin.By; "" for WorkBuddy
	label    string // the vendor in the log
	recheck  bool   // its campaigns can change before the Beijing day ends
	accounts func() []checkinAcct
}

// checkinRunner is a check-in the daily loop runs.
type checkinRunner interface {
	checkinNow(ctx context.Context, soon bool) []WorkBuddyCheckin
	clock() time.Time
}

func (c checkiner) clock() time.Time { return c.now() }

// checkinNow checks each account in use in for today that isn't yet, and
// keeps what came of it; soon, a failure is tried again without waiting
// out wbCheckinRetry. It says how every account stands, those it didn't
// ask as last kept.
func (c checkiner) checkinNow(ctx context.Context, soon bool) []WorkBuddyCheckin {
	if discoveryOff() {
		return nil
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	st := readCheckins(c.path)
	now := c.now()
	day := wbCheckinDay(now)
	var out []WorkBuddyCheckin
	changed := false
	// one account signed in twice (the built-in and the plugin) is
	// checked in once
	asked := map[string]bool{}
	// and an account the user left out is not asked at all, by the loop
	// or a press (#1902)
	off := checkinOff()
	for _, a := range c.accounts() {
		if !a.On || a.key == "" || asked[a.key] || off[a.key] {
			continue
		}
		// nor, by the loop, one whose provider is switched off or removed
		// (leftAlone); a press still checks it in
		if !soon && leftAlone(a.provider) {
			continue
		}
		asked[a.key] = true
		prev, seen := st[a.key]
		settled := prev.settled(day, now, soon)
		if c.recheck {
			settled = settled && !soon && now.Sub(prev.At) < wbCheckinEvery
			if prev.Checked() && !prev.ValidUntil.IsZero() {
				settled = settled && now.Before(prev.ValidUntil)
			}
		}
		if seen && settled {
			prev.User, prev.By = a.User, c.by
			out = append(out, prev)
			continue
		}
		r := a.do(ctx)
		r.Day, r.At = day, now
		if r.Offline && seen && prev.Day == day && prev.Offline {
			r.Tries = prev.Tries + 1
		}
		st[a.key], changed = r, true
		r.User, r.By, r.Asked = a.User, c.by, true
		out = append(out, r)
	}
	if changed {
		if b, err := json.MarshalIndent(st, "", "  "); err == nil {
			if err := writePrivate(c.path, append(b, '\n')); err != nil {
				log.Printf("%s check-in: %v", c.label, err)
			}
		}
	}
	return out
}

// checkinOff are the accounts left out of their vendor's daily check-in
// (settings.CheckinOff), by their entry in its file.
func checkinOff() map[string]bool {
	off := map[string]bool{}
	for _, k := range settings.Load().CheckinOff {
		off[k] = true
	}
	return off
}

// CheckinAccount is an account a daily check-in reaches, for Settings: its
// entry in its vendor's file (Key, which settings.CheckinOff names), the
// vendor (By, as WorkBuddyCheckin's), and whether it is checked in while
// its vendor's check-in is on (#1902, magicLaLa: 根据每个账号设置是否开启
// 自动签到).
type CheckinAccount struct {
	Key  string `json:"key"`
	By   string `json:"by"`
	User string `json:"user"`
	On   bool   `json:"on"`
}

// CheckinAccounts are the accounts in use that each vendor's check-in
// reaches, WorkBuddy's, Trae CN's, MiniMax Code's, Qoder's and the
// plugins' that check in themselves, each once; asks nothing.
func CheckinAccounts() []CheckinAccount {
	cs := []checkiner{
		newWBCheckiner().checkiner(),
		newTraeCheckiner(traeCheckinAccounts),
		newMiniMaxCheckiner(miniMaxCheckinAccounts),
		newQoderCheckiner(qoderCheckinAccounts, time.Now),
	}
	for _, p := range pluginCheckinProviders() {
		cs = append(cs, p.checkiner(time.Now))
	}
	off := checkinOff()
	var out []CheckinAccount
	seen := map[string]bool{}
	for _, c := range cs {
		var mine []CheckinAccount
		for _, a := range c.accounts() {
			if !a.On || a.key == "" || seen[c.by+"\x00"+a.key] {
				continue
			}
			seen[c.by+"\x00"+a.key] = true
			mine = append(mine, CheckinAccount{Key: a.key, By: c.by, User: a.User, On: !off[a.key]})
		}
		sort.SliceStable(mine, func(i, j int) bool { return mine[i].User < mine[j].User })
		out = append(out, mine...)
	}
	return out
}
