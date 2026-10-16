package provider

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"sync"
	"time"
)

// A remote magpie's quotas (莫 on Discord): a computer whose providers are
// another's magpie (remote-magpie) has the accounts signed in there, not
// here, so its Usage page, menu bar and magpie quota had nothing to show.
// They now show the other magpie's cards as it last read them, each named
// with that magpie ("Codex · office") and its provider id put after the
// remote's ("office/codex"), so they never meet this computer's own.
//
// Only the magpie holding the sign-ins asks the vendors. It answers
// RemoteCardsPath from what it has kept (CachedCards: its page's cards and
// what was read behind the requests since, cards_kept.go) and asks nobody;
// a card's refresh here asks it to read that card again
// (RemoteRefreshPath), which is the one time a vendor is asked for us.

// RemoteCardsPath is the gateway's answer of its kept cards.
const RemoteCardsPath = "/v1/magpie/quotas/cards"

// RemoteRefreshPath has the gateway read a card (?provider=, &user=)
// again, or every card when no provider is named, then answer as
// RemoteCardsPath.
const RemoteRefreshPath = "/v1/magpie/quotas/refresh"

// remoteCardsFor is how long another magpie's cards are kept here before
// they are asked for again: reading its cache costs it nothing from the
// vendors, but the menu bar and alerts ask often.
const remoteCardsFor = 30 * time.Second

// errRemoteNothingRead is a remote magpie that has read no card since it
// started: it is asked to only when a card is refreshed here.
const errRemoteNothingRead = "nothing read on that magpie yet; refresh to have it read"

// errRemoteNoShare is a remote magpie too old to answer RemoteCardsPath.
const errRemoteNoShare = "remote magpie doesn't share its quotas; update magpie on that computer"

// errRemoteNotShared is a remote magpie that answers models but not its
// quotas (403): it isn't shared on its local network, as an older magpie's
// gateway that MAGPIE_ADDR opened (Docker's) wasn't until it was (Fim980 on
// X: the card said only "Allowance unavailable"). A newer one asks a key of
// every other machine and, given one, answers the quotas too.
const errRemoteNotShared = "remote magpie isn't shared; turn on Share on local network on that computer"

// errRemoteKey is a remote magpie that didn't take this provider's key (401).
const errRemoteKey = "remote magpie didn't take this key; use one of its enabled gateway keys"

// KeyHeldCode is the error code a gateway answers a key held to a budget,
// models or accounts with, where its own accounts would be told (#1981):
// the key is someone else's to use, and is told its own limit alone.
const KeyHeldCode = "gateway_key_held"

// errKeyHeld is a remote magpie that holds this provider's key to a
// budget, models or accounts: it tells the key's own card (remoteKeyCards)
// and none of its accounts', so none is shown, not even one kept from
// before the key was held.
var errKeyHeld = errors.New("remote magpie holds this key to a budget, models or accounts")

// errRemoteUnreachable starts the error of a remote magpie that couldn't
// be reached at all.
const errRemoteUnreachable = "couldn't reach the remote magpie"

// CachedCards is the Usage page's cards as this magpie last read them,
// each with its kind, for another magpie to show: no vendor is asked,
// whatever their age, and another magpie's cards shown here aren't
// passed on.
//
// The cards are the page's with what was read of the same accounts and
// keys behind it since (cards_kept.go): a magpie serving only other
// magpies has nobody on its page (#1313).
func CachedCards(now time.Time) []SubscriptionQuota {
	c := &subscriptionUsageCache
	c.Lock()
	data := c.data
	c.Unlock()
	subs := withDailyCredits(visibleQuotas(withAccountReadings(data)), now)
	p := &planQuotaCache
	p.Lock()
	plans := slices.Clone(p.data)
	p.Unlock()
	b := &keyBalanceCache
	b.Lock()
	balances := slices.Clone(b.data)
	b.Unlock()
	ps := keyCardProviders()
	if plans == nil || balances == nil {
		keptPlans, keptBalances := keptKeyCards(ps)
		if plans == nil {
			plans = keptPlans
		}
		if balances == nil {
			balances = keptBalances
		}
	}
	plans, balances = withKeyReadings(plans, ps, "plan"), withKeyReadings(balances, ps, "balance")
	plans, subs = seatCards(plans, subs)
	subs = withAccountNames(subs)
	out := []SubscriptionQuota{}
	for _, g := range []struct {
		kind string
		qs   []SubscriptionQuota
	}{{"subscription", subs}, {"plan", plans}, {"balance", balances}} {
		for _, q := range g.qs {
			if q.From == "" {
				q.Kind = g.kind
				out = append(out, q)
			}
		}
	}
	return out
}

// ReadAllCards has every card read again, the page's Refresh asked by
// another magpie whose card of this one's said nothing was read yet.
func ReadAllCards(ctx context.Context) {
	AskUsage()
	ForgetBalances()
	forgetPlanQuotas()
	Quotas(Asked(ctx))
}

var remoteCardCache struct {
	sync.Mutex
	m map[string]remoteCardsRead
}

type remoteCardsRead struct {
	at    time.Time
	cards []SubscriptionQuota // as the remote has them, not yet named
	err   string
	held  bool // the remote holds the key: no card of its accounts is shown
}

// remoteCardsID is what a remote's cards are kept here by: the provider
// and its key, so cards read with one key are never shown under another
// (a key replaced by one the remote holds shows none of them).
func remoteCardsID(p Provider) string {
	return p.ID + "\x00" + keyID(p.Key)
}

// ForgetRemoteCardsForTest drops the remote magpies' cards kept here, for
// a test run again (-count) to ask its own fake remote afresh.
func ForgetRemoteCardsForTest() {
	c := &remoteCardCache
	c.Lock()
	c.m = nil
	c.Unlock()
	k := &remoteKeyCache
	k.Lock()
	k.m = nil
	k.Unlock()
}

// ForgetKeptCardsForTest has the cards kept on disk (quotas.json) read
// again from the home a test has set: they are read once per process.
func ForgetKeptCardsForTest() {
	c := &lastQuotas
	c.Lock()
	c.m, c.loaded = nil, false
	c.Unlock()
}

// remoteMagpies are the remote magpies this magpie uses.
func remoteMagpies() []Provider {
	var out []Provider
	for _, p := range All() {
		if p.IsRemoteMagpie() && !p.Hidden && !p.Off && p.Key != "" && p.Anthropic != "" {
			out = append(out, p)
		}
	}
	return out
}

// RemoteCards is every remote magpie's cards, named for this computer.
func RemoteCards(ctx context.Context) []SubscriptionQuota {
	ps := remoteMagpies()
	got := make([][]SubscriptionQuota, len(ps))
	var wg sync.WaitGroup
	for i, p := range ps {
		wg.Add(1)
		go func() {
			defer wg.Done()
			// its keys' own limits (#1547), read beside its cards
			keys := make(chan []SubscriptionQuota, 1)
			go func() { keys <- remoteKeyCards(ctx, p, false, "") }()
			got[i] = append(remoteCards(ctx, p, false), <-keys...)
		}()
	}
	wg.Wait()
	return slices.Concat(got...)
}

// remoteCards is p's cards, from what was kept here when it is recent and
// fresh isn't asked. A remote that can't be asked keeps showing its last
// cards, as old as they are; with none, one card says why.
func remoteCards(ctx context.Context, p Provider, fresh bool) []SubscriptionQuota {
	id := remoteCardsID(p)
	c := &remoteCardCache
	c.Lock()
	r, ok := c.m[id]
	c.Unlock()
	if !ok || fresh || time.Since(r.at) >= remoteCardsFor {
		asked := time.Now()
		cards, err := askRemote(ctx, p, http.MethodGet, RemoteCardsPath)
		r = keepRemoteCards(id, asked, cards, err)
	}
	name := remoteName(p)
	if r.held {
		return nil
	}
	if len(r.cards) == 0 {
		msg := r.err
		if msg == "" {
			msg = errRemoteNothingRead
		}
		return []SubscriptionQuota{{Provider: p.ID, Name: name, Icon: "magpie", Kind: "subscription", From: name, Windows: []QuotaWindow{}, Error: msg}}
	}
	out := make([]SubscriptionQuota, 0, len(r.cards))
	for _, q := range r.cards {
		if q.From != "" || q.Provider == "" {
			continue
		}
		q.Provider = p.ID + "/" + q.Provider
		q.Name += " · " + name
		q.From = name
		// what is pressed on a card acts on this computer's accounts:
		// a Codex reset or a check-in is the other magpie's to do
		q.Resets, q.Checkins, q.CheckinBy, q.Checkin, q.CheckinOff = nil, false, "", nil, false
		if q.Windows == nil {
			q.Windows = []QuotaWindow{}
		}
		out = append(out, q)
	}
	return out
}

func remoteName(p Provider) string {
	if p.Name != "" {
		return p.Name
	}
	return p.ID
}

// remoteOf is the remote magpie a card's provider id names, and the id of
// the card there ("" for the card that stands for the remote itself).
func remoteOf(id string) (Provider, string, bool) {
	rid, card, _ := strings.Cut(id, "/")
	for _, p := range remoteMagpies() {
		if p.ID == rid {
			return p, card, true
		}
	}
	return Provider{}, "", false
}

// IsRemoteCard is whether id is a card this magpie has from a remote
// magpie, or the card standing for the remote itself.
func IsRemoteCard(id string) bool {
	_, _, ok := remoteOf(id)
	return ok
}

// refreshRemote has the remote magpie read its card again (every card,
// for the remote's own), then keeps what it now has.
func refreshRemote(ctx context.Context, p Provider, card, user string) {
	// a key's limit card is read again with that key (or every key's)
	if card == remoteKeyCard {
		remoteKeyCards(ctx, p, true, user)
		return
	}
	q := url.Values{}
	if card != "" {
		q.Set("provider", card)
		if user != "" {
			q.Set("user", user)
		}
	}
	path := RemoteRefreshPath
	if len(q) > 0 {
		path += "?" + q.Encode()
	}
	asked := time.Now()
	if _, err := askRemote(ctx, p, http.MethodPost, path); err != nil {
		keepRemoteCards(remoteCardsID(p), asked, nil, err)
	}
	remoteCards(ctx, p, true)
}

// keepRemoteCards keeps what a read of a remote's cards asked at asked
// got, and gives back what is now kept. A failed read keeps the cards
// read before, with its error; a key the remote holds drops them. A read
// that comes back after one asked later is not kept over it, so a slow
// answer from before the key was held can't bring its cards back.
func keepRemoteCards(id string, asked time.Time, cards []SubscriptionQuota, err error) remoteCardsRead {
	c := &remoteCardCache
	c.Lock()
	defer c.Unlock()
	r := c.m[id]
	if asked.Before(r.at) {
		return r
	}
	r.at = asked
	switch {
	case errors.Is(err, errKeyHeld):
		r.cards, r.err, r.held = nil, "", true
	case err != nil:
		r.err = err.Error()
	default:
		r.cards, r.err, r.held = cards, "", false
	}
	if c.m == nil {
		c.m = map[string]remoteCardsRead{}
	}
	c.m[id] = r
	return r
}

// RemoteQuotaHistories is every remote magpie's quota history since days
// ago, as its own GET /v1/magpie/quotas/history has it (read from its
// disk, no vendor asked), the providers named as RemoteCards names them.
func RemoteQuotaHistories(ctx context.Context, days string) []QuotaHistory {
	ps := remoteMagpies()
	got := make([][]QuotaHistory, len(ps))
	var wg sync.WaitGroup
	for i, p := range ps {
		wg.Add(1)
		go func() {
			defer wg.Done()
			var hs []QuotaHistory
			if err := remoteJSON(ctx, p, http.MethodGet, "/v1/magpie/quotas/history?days="+url.QueryEscape(days), &hs); err != nil {
				return
			}
			for j := range hs {
				hs[j].Provider = p.ID + "/" + hs[j].Provider
			}
			got[i] = hs
		}()
	}
	wg.Wait()
	return slices.Concat(got...)
}

func askRemote(ctx context.Context, p Provider, method, path string) ([]SubscriptionQuota, error) {
	var cards []SubscriptionQuota
	if err := remoteJSON(ctx, p, method, path, &cards); err != nil {
		return nil, err
	}
	return cards, nil
}

// remoteJSON asks p's gateway, with its key, for a {"data": …} list.
func remoteJSON(ctx context.Context, p Provider, method, path string, dst any) error {
	ctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, method, p.Anthropic+path, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+p.Key)
	req.Header.Set("Accept", "application/json")
	res, err := p.Do(http.DefaultClient, req)
	if err != nil {
		return fmt.Errorf("%s %s: %w", errRemoteUnreachable, remoteName(p), err)
	}
	defer res.Body.Close()
	b, _ := io.ReadAll(io.LimitReader(res.Body, 8<<20))
	switch {
	case res.StatusCode == http.StatusNotFound || res.StatusCode == http.StatusMethodNotAllowed:
		return fmt.Errorf("%s", errRemoteNoShare)
	case res.StatusCode == http.StatusForbidden:
		var e struct {
			Error struct {
				Code string `json:"code"`
			} `json:"error"`
		}
		if json.Unmarshal(b, &e) == nil && e.Error.Code == KeyHeldCode {
			return errKeyHeld
		}
		return fmt.Errorf("%s", errRemoteNotShared)
	case res.StatusCode == http.StatusUnauthorized:
		return fmt.Errorf("%s", errRemoteKey)
	case res.StatusCode < 200 || res.StatusCode >= 300:
		var e struct {
			Error struct {
				Message string `json:"message"`
			} `json:"error"`
		}
		if json.Unmarshal(b, &e) == nil && e.Error.Message != "" {
			return fmt.Errorf("%s: %s", remoteName(p), e.Error.Message)
		}
		return fmt.Errorf("%s answered %s", remoteName(p), res.Status)
	}
	var list struct {
		Data json.RawMessage `json:"data"`
	}
	if err := json.Unmarshal(b, &list); err != nil {
		return err
	}
	if len(list.Data) == 0 {
		return nil
	}
	return json.Unmarshal(list.Data, dst)
}
