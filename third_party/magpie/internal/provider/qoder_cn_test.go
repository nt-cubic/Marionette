package provider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/qoder"
)

// qoderSiteClient is qoderTestClient keeping each request's real host, so a
// test can say which of Qoder's sites a call went to.
func qoderSiteClient(t *testing.T, h func(host string, w http.ResponseWriter, r *http.Request)) {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h(r.Header.Get("X-Test-Host"), w, r)
	}))
	t.Cleanup(srv.Close)
	u, _ := url.Parse(srv.URL)
	old := qoderClient
	qoderClient = &http.Client{Transport: qoderTransport(func(r *http.Request) (*http.Response, error) {
		r.Header.Set("X-Test-Host", r.URL.Host)
		r.URL.Scheme, r.URL.Host = u.Scheme, u.Host
		return srv.Client().Transport.RoundTrip(r)
	})}
	t.Cleanup(func() { qoderClient = old })
}

// qoderCNServer answers Qoder's endpoints as one site would, failing the test
// for a call to any other host; jobToken says whether the site makes job
// tokens or refuses them as Qoder CN might for its CLI's client id.
type qoderCNServer struct {
	t        *testing.T
	host     string // the account host; the model host is api
	api      string
	jobToken bool
	mu       sync.Mutex
	seen     []string
}

func (s *qoderCNServer) serve(host string, w http.ResponseWriter, r *http.Request) {
	path := r.URL.Path
	s.mu.Lock()
	s.seen = append(s.seen, host+path)
	s.mu.Unlock()
	want := s.host
	if strings.HasPrefix(path, "/algo/") {
		want = s.api
	}
	if host != want {
		s.t.Errorf("%s went to %s, want %s", path, host, want)
	}
	switch path {
	case qoder.DeviceTokenPollPath:
		_, _ = w.Write([]byte(`{"token":"dt-cn","refresh_token":"drt-cn","user_id":"u-cn","user_name":"CN","expires_in":7200}`))
	case qoder.JobTokenPath:
		var b map[string]string
		_ = json.NewDecoder(r.Body).Decode(&b)
		if s.host == "openapi.qoder.com.cn" && b["clientId"] != qoder.CNClientID {
			s.t.Errorf("job token clientId %q", b["clientId"])
		}
		if !s.jobToken {
			w.WriteHeader(http.StatusForbidden)
			_, _ = w.Write([]byte(`{"message":"client not allowed"}`))
			return
		}
		_, _ = w.Write([]byte(`{"token":"jt-cn","refresh_token":"rt-cn","expires_in":3600000}`))
	case qoder.JobTokenRefreshPath:
		_, _ = w.Write([]byte(`{"token":"jt-cn2","refresh_token":"rt-cn2","expires_in":3600000}`))
	case qoder.DeviceTokenRefreshPath:
		_, _ = w.Write([]byte(`{"token":"dt-cn2","refresh_token":"drt-cn2","expires_in":7200}`))
	case qoder.UserInfoPath:
		_, _ = w.Write([]byte(`{"id":"u-cn","email":"cn@x","name":"CN"}`))
	case qoder.AccountUsagePath:
		_, _ = w.Write([]byte(`{"displayMode":"qoder","qoderUsage":{"userType":"pro","userQuota":{"total":100,"used":25}}}`))
	case strings.Split(qoder.ListModelsPath, "?")[0]:
		_, _ = w.Write(qoderTestCredential("cn").Models)
	default:
		s.t.Errorf("unexpected request %s%s", host, path)
		w.WriteHeader(404)
	}
}

func (s *qoderCNServer) went(hostPath string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, x := range s.seen {
		if x == hostPath {
			return true
		}
	}
	return false
}

// TestQoderCNSignIn signs in to Qoder CN end to end against stand-ins for its
// hosts: the page on qoder.cn with Qoder CN's CLI client id, the poll, job
// token and user info on openapi.qoder.com.cn, the models on
// gateway.qoder.com.cn; then usage and a refresh stay on those hosts, and the
// account is Qoder CN's, not the global Qoder's.
func TestQoderCNSignIn(t *testing.T) {
	signIn(t)
	srv := &qoderCNServer{t: t, host: "openapi.qoder.com.cn", api: "gateway.qoder.com.cn", jobToken: true}
	qoderSiteClient(t, srv.serve)
	authURL, flow, err := qoderAuthURL(qoderSiteOf(QoderCNID))
	if err != nil {
		t.Fatal(err)
	}
	u, _ := url.Parse(authURL)
	if u.Scheme+"://"+u.Host != "https://qoder.cn" || u.Path != qoder.DeviceSelectAccountsPath {
		t.Fatalf("auth page %s", authURL)
	}
	q := u.Query()
	if q.Get("client_id") != qoder.CNClientID || q.Has("redirect_uri") || q.Get("challenge_method") != "S256" || q.Get("nonce") == "" || q.Get("machine_id") == "" {
		t.Fatalf("auth query %v", q)
	}
	who, err := QoderCompleteSignIn(context.Background(), flow)
	if err != nil {
		t.Fatal(err)
	}
	if who != "cn@x" || len(Logins(QoderCNID)) != 1 || len(Logins("qoder")) != 0 {
		t.Fatalf("sign-in %s: cn %+v global %+v", who, Logins(QoderCNID), Logins("qoder"))
	}
	for _, p := range []string{"openapi.qoder.com.cn" + qoder.DeviceTokenPollPath, "openapi.qoder.com.cn" + qoder.JobTokenPath,
		"openapi.qoder.com.cn" + qoder.UserInfoPath, "gateway.qoder.com.cn/algo/api/v2/model/list"} {
		if !srv.went(p) {
			t.Errorf("no call to %s: %v", p, srv.seen)
		}
	}
	cred, err := QoderCredentialOf(context.Background(), QoderCNID, who)
	if err != nil || cred.Site != QoderCNID || cred.DeviceChat || cred.Token != "jt-cn" || cred.OnSite().ChatURL() != "https://gateway.qoder.com.cn"+qoder.ChatPath {
		t.Fatalf("saved %+v %v", cred, err)
	}
	if _, err := QoderCredential(context.Background(), who); err == nil {
		t.Fatal("a Qoder CN account answers as a global one")
	}
	p, ok := qoderAccountOf(QoderCNID)
	if !ok || p.ID != QoderCNID || p.Name != "Qoder CN" || p.Website != "https://qoder.cn" || len(p.Account.models()) == 0 || p.Account.models()[0].Provider != QoderCNID {
		t.Fatalf("provider %+v", p)
	}
	if _, ok := qoderAccount(); ok {
		t.Fatal("Qoder CN account listed as the global Qoder")
	}

	quota := qoderLoginQuota(context.Background(), Login{Agent: QoderCNID, User: who})
	if quota.Error != "" || quota.Provider != QoderCNID || quota.Name != "Qoder CN" || len(quota.Windows) != 1 || !srv.went("openapi.qoder.com.cn"+qoder.AccountUsagePath) {
		t.Fatalf("usage %+v", quota)
	}

	// an expiring job token is refreshed on Qoder CN's account host
	c := *cred
	c.ExpiresAt = time.Now().Add(time.Minute).UnixMilli()
	if err := qoderSave(c); err != nil {
		t.Fatal(err)
	}
	fresh, err := QoderCredentialOf(context.Background(), QoderCNID, who)
	if err != nil || fresh.Token != "jt-cn2" || fresh.RefreshToken != "rt-cn2" || !srv.went("openapi.qoder.com.cn"+qoder.JobTokenRefreshPath) {
		t.Fatalf("refresh %+v %v", fresh, err)
	}
	if err := forgetQoderLogin(QoderCNID, who); err != nil || len(Logins(QoderCNID)) != 0 {
		t.Fatalf("forget %v", err)
	}
}

// TestQoderCNDeviceChat: when qoder.cn won't trade the device token for a job
// token, the account works as Qoder CN's CLI does, the device token signing
// the model calls, and is refreshed as a device token.
func TestQoderCNDeviceChat(t *testing.T) {
	signIn(t)
	srv := &qoderCNServer{t: t, host: "openapi.qoder.com.cn", api: "gateway.qoder.com.cn"}
	qoderSiteClient(t, srv.serve)
	_, flow, err := qoderAuthURL(qoderSiteOf(QoderCNID))
	if err != nil {
		t.Fatal(err)
	}
	who, err := QoderCompleteSignIn(context.Background(), flow)
	if err != nil {
		t.Fatal(err)
	}
	cred, err := QoderCredentialOf(context.Background(), QoderCNID, who)
	if err != nil || !cred.DeviceChat || cred.Token != "dt-cn" || cred.RefreshToken != "drt-cn" || len(cred.Models) == 0 {
		t.Fatalf("saved %+v %v", cred, err)
	}
	if left := time.Until(time.UnixMilli(cred.ExpiresAt)); left < time.Hour || left > 2*time.Hour {
		t.Fatalf("device token lifetime %v", left)
	}
	c := *cred
	c.ExpiresAt = time.Now().Add(time.Minute).UnixMilli()
	if err := qoderSave(c); err != nil {
		t.Fatal(err)
	}
	fresh, err := QoderCredentialOf(context.Background(), QoderCNID, who)
	if err != nil || fresh.Token != "dt-cn2" || fresh.DeviceToken != "dt-cn2" || fresh.RefreshToken != "drt-cn2" || fresh.DeviceRefresh != "drt-cn2" ||
		!srv.went("openapi.qoder.com.cn"+qoder.DeviceTokenRefreshPath) || srv.went("openapi.qoder.com.cn"+qoder.JobTokenRefreshPath) {
		t.Fatalf("refresh %+v %v %v", fresh, err, srv.seen)
	}
}

// TestQoderGlobalUnchanged: the global site is what it was — qoder.com with
// the desktop client id and redirect, openapi.qoder.sh and api3.qoder.sh, no
// fallback when a job token is refused — and a credential saved before there
// were two sites, with no site in it, is a global one.
func TestQoderGlobalUnchanged(t *testing.T) {
	signIn(t)
	srv := &qoderCNServer{t: t, host: "openapi.qoder.sh", api: "api3.qoder.sh", jobToken: true}
	qoderSiteClient(t, srv.serve)
	authURL, flow, err := QoderAuthURL()
	if err != nil {
		t.Fatal(err)
	}
	u, _ := url.Parse(authURL)
	if u.Host != "qoder.com" || u.Query().Get("client_id") != qoder.ClientID || u.Query().Get("redirect_uri") != qoder.RedirectURI {
		t.Fatalf("global auth page %s", authURL)
	}
	who, err := QoderCompleteSignIn(context.Background(), flow)
	if err != nil || len(Logins("qoder")) != 1 || len(Logins(QoderCNID)) != 0 {
		t.Fatalf("global sign-in %s %v", who, err)
	}
	loginsMu.Lock()
	raw := readLogins()[0].Auth
	loginsMu.Unlock()
	if strings.Contains(string(raw), `"site"`) || strings.Contains(string(raw), `"device_chat"`) {
		t.Fatalf("global credential gained fields: %s", raw)
	}

	legacy := qoderTestCredential("old")
	legacy.ExpiresAt = time.Now().Add(time.Minute).UnixMilli()
	if legacy.Site != "" || legacy.OnSite() != qoder.Global {
		t.Fatal("a credential without a site isn't global")
	}
	if err := qoderSave(legacy); err != nil {
		t.Fatal(err)
	}
	fresh, err := QoderCredential(context.Background(), "old@x")
	if err != nil || fresh.Token != "jt-cn2" || !srv.went("openapi.qoder.sh"+qoder.JobTokenRefreshPath) || fresh.OnSite().ChatURL() != qoder.ChatURL() {
		t.Fatalf("legacy refresh %+v %v", fresh, err)
	}

	srv.jobToken = false
	_, flow, _ = QoderAuthURL()
	if _, err := QoderCompleteSignIn(context.Background(), flow); err == nil {
		t.Fatal("a refused job token signed in to the global site")
	}
}
