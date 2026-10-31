package provider

// Keeping saved accounts signed in. A ChatGPT account saved in magpie but
// not the one Codex is signed in to is refreshed only when it is used or
// its allowance looked at — and one waiting its turn (smart
// routing leaves the accounts whose week resets later for days) could sit
// unused past its refresh token's life, and be found signed out just when
// it's needed. So whichever magpie runs the gateway renews each such
// sign-in about as often as Codex renews its own: every eight days, one
// account an hour, never all of them at once. The account the agent is signed in to is the
// agent's own to refresh, and is left to it. A Claude account is never
// refreshed by magpie: Claude Code refreshes it, whenever it runs on it
// (claude_dirs.go).

import (
	"context"
	"encoding/json"
	"errors"
	"log"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// keepAliveEvery is how long a saved sign-in goes before it is renewed:
// Codex's own TOKEN_REFRESH_INTERVAL (codex-rs login/src/auth/manager.rs,
// 8 days). Once a day renewed every idle account eight times as often as
// Codex does, each one in the same hourly look as the others: a burst of
// N refreshes from one address every day (owl on Discord).
const keepAliveEvery = 8 * 24 * time.Hour

// keepAliveAtOnce is how many due sign-ins one look renews; the rest wait
// for the next look, an hour on, so accounts saved together drift apart
// instead of being renewed side by side for good.
const keepAliveAtOnce = 1

// Renewal is what came of renewing one saved account.
type Renewal struct {
	Agent string `json:"agent"`
	User  string `json:"user"`
	// Renewed is false when it wasn't due; Err is why it failed, and Lapsed
	// whether the sign-in is gone (the vendor refused it) or may yet renew.
	Renewed bool   `json:"renewed"`
	Err     string `json:"error,omitempty"`
	Lapsed  bool   `json:"lapsed,omitempty"`
}

// RenewLogins renews the saved ChatGPT sign-ins not in the
// agent's hands that have gone every or longer without it; every 0 renews
// all of them.
func RenewLogins(ctx context.Context, every time.Duration) []Renewal {
	return renewLogins(ctx, every, 0)
}

// renewLogins is RenewLogins renewing at most atOnce of the due sign-ins
// (0: all of them); a due one past that is left for the next look and
// reported as not renewed.
func renewLogins(ctx context.Context, every time.Duration, atOnce int) []Renewal {
	if settings.AccountDiscoveryOff() {
		return nil
	}
	rememberLogins(false)
	loginsMu.Lock()
	active := map[string]string{}
	for _, a := range loginAgents {
		if l, ok := liveLogin(a); ok {
			active[a] = l.User
		}
	}
	ls := readLogins()
	loginsMu.Unlock()
	out := []Renewal{}
	tried := 0
	for _, l := range ls {
		if l.Agent != "codex" || strings.EqualFold(active[l.Agent], l.User) || leftAlone(l.Agent) {
			continue
		}
		r := Renewal{Agent: l.Agent, User: l.User}
		if every > 0 && !renewalDue(l, every, time.Now()) || atOnce > 0 && tried >= atOnce {
			out = append(out, r)
			continue
		}
		tried++
		why := ctx
		if every > 0 {
			why = refreshingFor(ctx, codexRefreshScheduled)
		}
		if _, _, err := renewSavedLogin(why, l.Agent, l.User, true); err != nil {
			r.Err = err.Error()
			var refused refreshRefused
			r.Lapsed = errors.As(err, &refused)
		} else {
			r.Renewed = true
		}
		out = append(out, r)
	}
	return out
}

// renewalDue says whether a saved sign-in has gone every without being
// renewed — since magpie last renewed it, or since it was last seen in the
// agent's hands, which kept it fresh — or its refresh token is near its end.
func renewalDue(l savedLogin, every time.Duration, now time.Time) bool {
	last := l.Renewed
	if l.Seen.After(last) {
		last = l.Seen
	}
	var a struct {
		LastRefresh time.Time `json:"last_refresh"`
	}
	if json.Unmarshal(l.Auth, &a) == nil && a.LastRefresh.After(last) {
		last = a.LastRefresh
	}
	return now.Sub(last) >= every
}

// KeepLoginsAlive renews a saved sign-in that is due, one a look, a minute
// after it starts and every hour after that — a computer that slept through a day
// catches up within the hour of waking — until ctx ends.
func KeepLoginsAlive(ctx context.Context) {
	t := time.NewTimer(time.Minute)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
		c, cancel := context.WithTimeout(ctx, 2*time.Minute)
		for _, r := range renewLogins(c, keepAliveEvery, keepAliveAtOnce) {
			if r.Err != "" {
				log.Printf("keeping %s account %s signed in: %s", r.Agent, r.User, r.Err)
			}
		}
		cancel()
		t.Reset(time.Hour)
	}
}
