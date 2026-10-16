package provider

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// mimoAccountStub stands in for account.xiaomi.com: the long-poll sign-in
// (its ticket, and the poll answering with the passToken on its second ask)
// and serviceLogin, which sends a browser holding a good passToken back to
// the MiMo server's /sts and one without to its sign-in page.
type mimoAccountStub struct {
	t      *testing.T
	url    string
	polls  atomic.Int32
	refuse atomic.Bool // the passToken no longer signs anything on
}

func (a *mimoAccountStub) serve(w http.ResponseWriter, r *http.Request) {
	t := a.t
	if !strings.HasPrefix(r.Header.Get("User-Agent"), "miNative PC/") {
		t.Errorf("%s User-Agent %q", r.URL.Path, r.Header.Get("User-Agent"))
	}
	switch r.URL.Path {
	case "/longPolling/loginUrl":
		q := r.URL.Query()
		if q.Get("sid") != "mimosgp" || !strings.HasSuffix(q.Get("callback"), "/api/sts") || q.Get("qs") != "%3Fsid%3Dmimosgp%26_json%3Dtrue" {
			t.Errorf("loginUrl asked %s", r.URL.RawQuery)
		}
		if c, err := r.Cookie("deviceId"); err != nil || !strings.HasPrefix(c.Value, "pc_") {
			t.Errorf("loginUrl without the device id")
		}
		fmt.Fprintf(w, `&&&START&&&{"code":0,"loginUrl":%q,"qr":%q,"lp":%q,"timeout":300}`,
			a.url+"/pass/qr/login?ticket=t1", a.url+"/qr.png", a.url+"/lp/s?k=t1")
	case "/lp/s":
		if a.polls.Add(1) == 1 {
			// held, then let go with nothing yet
			_, _ = io.WriteString(w, `&&&START&&&{"code":700,"desc":"waiting"}`)
			return
		}
		_, _ = io.WriteString(w, `&&&START&&&{"code":0,"userId":42,"cUserId":"cu-42","passToken":"pass-42","ssecurity":"s","location":"https://x/sts"}`)
	case "/pass/serviceLogin":
		c, err := r.Cookie("passToken")
		if err != nil || c.Value != "pass-42" || a.refuse.Load() {
			w.Header().Set("Content-Type", "text/html")
			_, _ = io.WriteString(w, "<html>Xiaomi account sign-in</html>")
			return
		}
		if u, err := r.Cookie("userId"); err != nil || u.Value != "42" {
			t.Errorf("serviceLogin without the account's id")
		}
		back, _ := url.Parse(r.URL.Query().Get("callback"))
		q := back.Query()
		q.Set("nonce", "n1")
		q.Set("followup", r.URL.Query().Get("followup"))
		back.RawQuery = q.Encode()
		http.Redirect(w, r, back.String(), http.StatusFound)
	default:
		t.Errorf("account: unexpected %s %s", r.Method, r.URL.Path)
		w.WriteHeader(404)
	}
}

// mimoServerStub stands in for one MiMo server: /user/xiaomi/me sends a
// request with no session to Xiaomi's serviceLogin, /sts sets a session,
// and the account's pages answer only to the latest session.
type mimoServerStub struct {
	t        *testing.T
	acct     *mimoAccountStub
	base     string
	region   string // what /user/xiaomi/me says the account's region is
	mu       sync.Mutex
	token    string
	sessions atomic.Int32
}

func (m *mimoServerStub) signed(r *http.Request) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	c, err := r.Cookie("serviceToken")
	return err == nil && m.token != "" && c.Value == m.token
}

func (m *mimoServerStub) drop() {
	m.mu.Lock()
	m.token = "gone"
	m.mu.Unlock()
}

func (m *mimoServerStub) serve(w http.ResponseWriter, r *http.Request) {
	t := m.t
	if r.Header.Get("X-Client-Version") != mimoAppVersion {
		t.Errorf("%s X-Client-Version %q", r.URL.Path, r.Header.Get("X-Client-Version"))
	}
	switch r.URL.Path {
	case "/api/user/xiaomi/me":
		if !m.signed(r) {
			q := url.Values{"callback": {m.base + "/sts"}, "followup": {m.base + "/user/xiaomi/me"}, "sid": {"mimosgp"}, "_group": {"DEFAULT"}}
			http.Redirect(w, r, m.acct.url+"/pass/serviceLogin?"+q.Encode(), http.StatusFound)
			return
		}
		fmt.Fprintf(w, `{"code":0,"data":{"userId":42,"nickname":"Mi Fan","region":%q,"country":"SG"}}`, m.region)
	case "/api/sts":
		if r.URL.Query().Get("nonce") != "n1" {
			t.Errorf("/sts without Xiaomi's nonce")
		}
		n := m.sessions.Add(1)
		m.mu.Lock()
		m.token = fmt.Sprintf("st-%d", n)
		tok := m.token
		m.mu.Unlock()
		http.SetCookie(w, &http.Cookie{Name: "serviceToken", Value: tok, Path: "/"})
		http.SetCookie(w, &http.Cookie{Name: "userId", Value: "42", Path: "/"})
		http.Redirect(w, r, r.URL.Query().Get("followup"), http.StatusFound)
	case "/api/user/usage", "/api/user/xiaomi/subscription/self":
		if !m.signed(r) {
			w.WriteHeader(401)
			return
		}
		if r.URL.Path == "/api/user/usage" {
			_, _ = io.WriteString(w, `{"code":0,"data":{"percent":70,"resetDate":"2026-10-05"}}`)
			return
		}
		_, _ = io.WriteString(w, `{"code":0,"data":{"current":{"planCode":"mimo_pro_m","title":"MiMo 高阶","planTier":3,"status":"ACTIVE","renewalMode":"MONTHLY","endTime":"2026-11-01T00:00:00","source":"ORDER_SUB"}}}`)
	default:
		t.Errorf("mimo: unexpected %s %s", r.Method, r.URL.Path)
		w.WriteHeader(404)
	}
}

// mimoStubs points magpie at a fake account.xiaomi.com (as localhost, so
// its cookies are kept apart from the servers') and fake MiMo servers, one
// per region given; the account says it is in the first region's.
func mimoStubs(t *testing.T, regions ...string) (*mimoAccountStub, map[string]*mimoServerStub) {
	t.Helper()
	acct := &mimoAccountStub{t: t}
	as := httptest.NewServer(http.HandlerFunc(acct.serve))
	t.Cleanup(as.Close)
	acct.url = strings.Replace(as.URL, "127.0.0.1", "localhost", 1)
	oldHosts, oldSite := mimoHosts, mimoAccountSite
	t.Cleanup(func() { mimoHosts, mimoAccountSite = oldHosts, oldSite })
	mimoAccountSite = acct.url
	mimoHosts = map[string]string{}
	servers := map[string]*mimoServerStub{}
	for _, r := range regions {
		m := &mimoServerStub{t: t, acct: acct, region: regions[0]}
		s := httptest.NewServer(http.HandlerFunc(m.serve))
		t.Cleanup(s.Close)
		m.base = s.URL + "/api"
		mimoHosts[r] = m.base
		servers[r] = m
	}
	return acct, servers
}

func mimoSignInFor(t *testing.T) string {
	t.Helper()
	st, err := StartSignIn(MiMoID)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(st.URL, "/pass/qr/login?ticket=t1") {
		t.Fatalf("sign-in opens %q, want Xiaomi's page for the ticket", st.URL)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	done, err := WaitSignIn(ctx, st.ID)
	if err != nil {
		t.Fatal(err)
	}
	if done.State != "done" || done.User != "42" {
		t.Fatalf("sign-in ended %+v", done)
	}
	return done.User
}

func TestMiMoSignInRoutesAndMeters(t *testing.T) {
	signIn(t)
	acct, servers := mimoStubs(t, "SGP")
	sgp := servers["SGP"]
	user := mimoSignInFor(t)
	if acct.polls.Load() != 2 {
		t.Errorf("polled %d times, want 2 (waited past the first answer)", acct.polls.Load())
	}

	c, ok := mimoLookup(user)
	if !ok || c.PassToken != "pass-42" || c.CUserID != "cu-42" || c.Base != sgp.base || c.Region != "SGP" || c.Name != "Mi Fan" || c.Cookies["serviceToken"] != "st-1" {
		t.Fatalf("saved %+v", c)
	}
	var got *Login
	for _, l := range Logins(MiMoID) {
		if l.User == user {
			got = &l
		}
	}
	if got == nil || got.Plan != "MiMo 高阶" || !got.Active {
		t.Fatalf("logins %+v", Logins(MiMoID))
	}

	var p Provider
	for _, a := range Accounts() {
		if a.ID == MiMoID {
			p = a
		}
	}
	if p.Account == nil || p.Chat != sgp.base+"/route" || len(p.Account.models()) != 2 {
		t.Fatalf("provider %+v", p)
	}
	req := httptest.NewRequest(http.MethodPost, p.Chat+"/chat/completions", nil)
	req.Header.Set("Authorization", "Bearer magpie")
	if err := p.Sign(context.Background(), req, Chat, nil); err != nil {
		t.Fatal(err)
	}
	if req.Header.Get("Authorization") != "" || !strings.Contains(req.Header.Get("Cookie"), "serviceToken=st-1") ||
		req.Header.Get("X-Mimo-Source") != "mimocode-cli-free" || req.Header.Get("X-Client-Version") != mimoAppVersion {
		t.Fatalf("signed %v", req.Header)
	}
	if b := string(p.Prepare([]byte(`{"model":"mimo-auto","stream":true}`))); !strings.Contains(b, `"model":"mimo-pro"`) || !strings.Contains(b, `"stream":true`) {
		t.Fatalf("mimo-auto asked as %s", b)
	}
	if b := string(p.Prepare([]byte(`{"model":"mimo-flash"}`))); b != `{"model":"mimo-flash"}` {
		t.Fatalf("mimo-flash asked as %s", b)
	}

	q := mimoLoginQuota(context.Background(), *got)
	if q.Error != "" || q.Plan != "MiMo 高阶" || q.Renew != "auto" || len(q.Windows) != 1 {
		t.Fatalf("quota %+v", q)
	}
	w := q.Windows[0]
	if w.Used != 30 || w.Span != 7*24*time.Hour || w.ResetsAt == nil || !w.ResetsAt.Equal(time.Date(2026, 10, 5, 0, 0, 0, 0, mimoZone)) {
		t.Fatalf("window %+v", w)
	}
	if q.Until == nil || !q.Until.Equal(time.Date(2026, 11, 1, 0, 0, 0, 0, mimoZone)) {
		t.Fatalf("until %v", q.Until)
	}
	if sgp.sessions.Load() != 1 {
		t.Errorf("signed on %d times, want once", sgp.sessions.Load())
	}
}

// A session the server turns away is renewed once through the passToken;
// an old one before it is used; and when Xiaomi no longer takes the
// passToken, the account is marked to be signed in again.
func TestMiMoRenewsAndLapses(t *testing.T) {
	signIn(t)
	acct, servers := mimoStubs(t, "SGP")
	sgp := servers["SGP"]
	user := mimoSignInFor(t)

	sgp.drop()
	if _, _, err := mimoPlan(context.Background(), user); err != nil {
		t.Fatalf("plan after the session was dropped: %v", err)
	}
	if sgp.sessions.Load() != 2 {
		t.Fatalf("signed on %d times, want 2", sgp.sessions.Load())
	}
	if c, _ := mimoLookup(user); c.Cookies["serviceToken"] != "st-2" {
		t.Fatalf("renewed session not kept: %v", c.Cookies["serviceToken"])
	}

	// a day old: renewed before the request is signed
	if err := mimoEdit(user, func(c *mimoCreds, _ *savedLogin) { c.Issued = time.Now().Add(-25 * time.Hour) }); err != nil {
		t.Fatal(err)
	}
	p, _ := mimoAccount()
	req := httptest.NewRequest(http.MethodPost, p.Chat+"/chat/completions", nil)
	if err := p.Sign(context.Background(), req, Chat, nil); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(req.Header.Get("Cookie"), "serviceToken=st-3") {
		t.Fatalf("an old session was used: %q", req.Header.Get("Cookie"))
	}

	acct.refuse.Store(true)
	sgp.drop()
	l := Logins(MiMoID)[0]
	q := mimoLoginQuota(context.Background(), l)
	if !strings.Contains(q.Error, "sign in again") {
		t.Fatalf("quota with a refused passToken: %+v", q)
	}
	if l := Logins(MiMoID)[0]; l.Lapsed == "" {
		t.Fatalf("not marked lapsed: %+v", l)
	}
	if _, err := mimoFresh(context.Background(), user, true); !errors.Is(err, ErrMiMoSignIn) {
		t.Fatalf("renewing a refused sign-in: %v", err)
	}
}

// An account served at another region's server is moved there, as the app
// moves it.
func TestMiMoSignInMovesToItsRegion(t *testing.T) {
	signIn(t)
	_, servers := mimoStubs(t, "RU", "SGP")
	user := mimoSignInFor(t)
	c, _ := mimoLookup(user)
	if c.Region != "RU" || c.Base != servers["RU"].base || servers["RU"].sessions.Load() != 1 {
		t.Fatalf("saved at %s %s, RU signed on %d times", c.Region, c.Base, servers["RU"].sessions.Load())
	}
	if p, _ := mimoAccount(); p.Chat != servers["RU"].base+"/route" {
		t.Fatalf("routed to %s", p.Chat)
	}
}
