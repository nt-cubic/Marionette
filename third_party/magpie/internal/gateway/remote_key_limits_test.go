package gateway

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/budget"
	"github.com/yetone/magpie/internal/provider"
)

// #1547 (PennyVibe), between two magpies: a computer with another magpie
// as its provider, through two gateway keys that magpie gave it, is shown
// each key's own limit and what it used, read with that key alone. The
// numbers move with the key's own requests, never with the other key's;
// a key held to no limit says so.
func TestRemoteMagpieKeyLimitCards(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	budget.Forget()
	now := pinLimitClock(t)
	limitedUpstream(t, 0)
	keys, secrets := newCaller(t, "Laptop", "Desk")
	setLimit(t, keys[0].ID, &access.Limit{Period: "day", Tokens: 1000})
	setLimit(t, keys[1].ID, &access.Limit{Period: "week", Tokens: 5000})
	gw := httptest.NewServer(lanGuard(New().Handler()))
	defer gw.Close()
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset, Key: secrets[0], KeyName: "laptop",
		Keys: []provider.KeyAccount{{Name: "desk", Key: secrets[1]}}, Chat: gw.URL}); err != nil {
		t.Fatal(err)
	}
	provider.ForgetRemoteCardsForTest()
	t.Cleanup(provider.ForgetRemoteCardsForTest)
	ctx := context.Background()
	cards := func() map[string]provider.SubscriptionQuota {
		out := map[string]provider.SubscriptionQuota{}
		for _, q := range provider.RemoteCards(ctx) {
			if q.Provider == "office/~keys" {
				out[q.User] = q
			}
		}
		return out
	}
	tokens := func(q provider.SubscriptionQuota) float64 {
		t.Helper()
		if q.Error != "" || q.GatewayKey == nil || len(q.Windows) != 1 {
			t.Fatalf("card: %+v", q)
		}
		return q.Windows[0].Amount
	}
	cs := cards()
	if len(cs) != 2 || tokens(cs["laptop"]) != 0 || cs["laptop"].Windows[0].Limit != 1000 || cs["laptop"].Windows[0].Name != "Tokens today" ||
		tokens(cs["desk"]) != 0 || cs["desk"].Windows[0].Limit != 5000 || cs["desk"].Windows[0].Name != "Tokens this week" {
		t.Fatalf("before any request: %+v", cs)
	}
	chat := func(secret string) {
		t.Helper()
		r, _ := http.NewRequest("POST", gw.URL+"/v1/chat/completions", strings.NewReader(chatReq))
		r.Header.Set("Authorization", "Bearer "+secret)
		res, err := http.DefaultClient.Do(r)
		if err != nil {
			t.Fatal(err)
		}
		res.Body.Close()
		if res.StatusCode != 200 {
			t.Fatal(res.Status)
		}
	}
	// a request is counted once its reply is written, so a little
	// after the client has it
	settled := func(k access.Key, calls int) {
		t.Helper()
		all, _ := access.List()
		for _, a := range all {
			if a.ID == k.ID {
				k = a
			}
		}
		for start := time.Now(); time.Since(start) < 10*time.Second; time.Sleep(10 * time.Millisecond) {
			if st := budget.Of(k, now); st != nil && st.Calls == calls && st.InFlight == 0 {
				return
			}
		}
		t.Fatalf("%s never counted %d calls", k.Name, calls)
	}
	chat(secrets[0])
	chat(secrets[1])
	chat(secrets[1])
	settled(keys[0], 1)
	settled(keys[1], 2)
	provider.RefreshUsage(ctx, "office/~keys", "")
	cs = cards()
	if l := cs["laptop"]; tokens(l) != 350 || *l.GatewayKey.TokensLeft != 650 || l.GatewayKey.Calls != 1 || l.GatewayKey.Period != "day" || l.Windows[0].ResetsAt == nil {
		t.Errorf("laptop after its request: %+v %+v", l, l.GatewayKey)
	}
	if d := cs["desk"]; tokens(d) != 700 || *d.GatewayKey.TokensLeft != 4300 || d.GatewayKey.Calls != 2 {
		t.Errorf("desk after its requests: %+v %+v", d, d.GatewayKey)
	}
	// one key's refresh reads that key only
	chat(secrets[1])
	settled(keys[1], 3)
	provider.RefreshUsage(ctx, "office/~keys", "laptop")
	if d := cards()["desk"]; tokens(d) != 700 {
		t.Errorf("desk read again by laptop's refresh: %v", tokens(d))
	}
	// no limit
	setLimit(t, keys[0].ID, nil)
	provider.RefreshUsage(ctx, "office/~keys", "laptop")
	if l := cards()["laptop"]; l.GatewayKey == nil || l.GatewayKey.Limited || len(l.Windows) != 1 || !l.Windows[0].Unlimited {
		t.Errorf("laptop with no limit: %+v", l)
	}
	// every 5 hours (#2102): the card names the cycle's length
	setLimit(t, keys[0].ID, &access.Limit{Period: "hours", Hours: 5, Tokens: 1000})
	provider.RefreshUsage(ctx, "office/~keys", "laptop")
	if l := cards()["laptop"]; tokens(l) != 0 || l.GatewayKey.Period != "hours" || l.GatewayKey.Hours != 5 || l.GatewayKey.Days != 0 ||
		l.Windows[0].Name != "Tokens this cycle" || !l.GatewayKey.Reset.Equal(l.GatewayKey.Start.Add(5*time.Hour)) {
		t.Errorf("laptop every 5 hours: %+v %+v", l, l.GatewayKey)
	}
	// and a week beside it (#2227): a window for each, the key told of the
	// one holding it most
	n := 1
	if _, err := access.Update("limit-key", access.Change{Key: keys[0].ID, Limit: &access.Limit{Period: "week", Tokens: 300}, Index: &n}); err != nil {
		t.Fatal(err)
	}
	provider.RefreshUsage(ctx, "office/~keys", "laptop")
	if l := cards()["laptop"]; len(l.Windows) != 2 || l.Windows[0].Name != "Tokens this cycle" || l.Windows[1].Name != "Tokens this week" ||
		l.Windows[1].Amount != 350 || l.Windows[1].Limit != 300 || l.GatewayKey.Period != "week" || !l.GatewayKey.Spent {
		t.Errorf("laptop with two limits: %+v %+v", l, l.GatewayKey)
	}
}

// A key asking after its limit is told its own, whatever the request
// names: nothing in it chooses another key (#1547).
func TestKeyLimitTellsOnlyTheCaller(t *testing.T) {
	fresh(t)
	budget.Forget()
	pinLimitClock(t)
	limitedUpstream(t, 0)
	keys, secrets := newCaller(t, "A", "B")
	setLimit(t, keys[0].ID, &access.Limit{Period: "day", Tokens: 1000})
	setLimit(t, keys[1].ID, &access.Limit{Period: "week", Tokens: 9000})
	h := New().Handler()
	r := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(chatReq))
	r.Header.Set("Authorization", "Bearer "+secrets[1])
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	for _, q := range []string{"", "?key=" + keys[1].ID, "?key=B", "?id=" + keys[1].ID + "&name=B"} {
		r := httptest.NewRequest("GET", "/v1/magpie/limit"+q, nil)
		r.Header.Set("Authorization", "Bearer "+secrets[0])
		r.Header.Set("X-Magpie-Key", keys[1].ID)
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		var got struct {
			Limited bool
			Key     string
			Limit   budget.Status
		}
		json.Unmarshal(w.Body.Bytes(), &got)
		if w.Code != 200 || got.Key != "A" || !got.Limited || got.Limit.TokenLimit != 1000 || got.Limit.Tokens != 0 || got.Limit.Period != "day" {
			t.Errorf("%q: %d %s", q, w.Code, w.Body.String())
		}
		if strings.Contains(w.Body.String(), keys[1].ID) || strings.Contains(w.Body.String(), "9000") {
			t.Errorf("%q told B's: %s", q, w.Body.String())
		}
	}
	// without a key, from another machine: nothing
	r = httptest.NewRequest("GET", "/v1/magpie/limit", nil)
	r.RemoteAddr = "192.168.1.9:5000"
	w = httptest.NewRecorder()
	lanGuard(h).ServeHTTP(w, r)
	if w.Code == 200 {
		t.Errorf("no key from another machine: %d %s", w.Code, w.Body.String())
	}
}
