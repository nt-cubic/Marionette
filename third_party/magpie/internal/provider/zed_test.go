package provider

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

// zedCloudStub stands in for cloud.zed.dev (and zed.dev's sign-in page): it
// checks every /client/* call is signed with the account's pair and the
// machine id it was issued with, and every /models call with the last model
// token it minted.
type zedCloudStub struct {
	t        *testing.T
	minted   atomic.Int32
	refuse   atomic.Bool // llm_tokens answers 401: the sign-in is gone
	plan     string
	systemID string
}

func (z *zedCloudStub) serve(w http.ResponseWriter, r *http.Request) {
	t := z.t
	switch r.URL.Path {
	case "/client/users/me", "/client/llm_tokens":
		if r.Header.Get("Authorization") != "4242 plain-access" {
			t.Errorf("%s signed %q", r.URL.Path, r.Header.Get("Authorization"))
		}
		if !strings.HasPrefix(r.Header.Get("User-Agent"), "Zed/") {
			t.Errorf("%s User-Agent %q", r.URL.Path, r.Header.Get("User-Agent"))
		}
		if z.systemID != "" && r.Header.Get("x-zed-system-id") != z.systemID {
			t.Errorf("%s from system %q, want %q", r.URL.Path, r.Header.Get("x-zed-system-id"), z.systemID)
		}
		if r.URL.Path == "/client/users/me" {
			_, _ = io.WriteString(w, `{"user":{"legacy_user_id":4242,"github_login":"octo","name":"Octo Cat"},
				"organizations":[{"id":"org-team","name":"Team"},{"id":"org-me","name":"Me","is_personal":true}],
				"default_organization_id":"org-me","plans_by_organization":{"org-me":"`+z.plan+`"},
				"plan":{"plan_v3":"`+z.plan+`","subscription_period":{"started_at":"2026-09-01T00:00:00Z","ended_at":"2026-10-01T00:00:00Z"},"has_overdue_invoices":false}}`)
			return
		}
		if z.refuse.Load() {
			w.WriteHeader(401)
			return
		}
		var body struct {
			Org string `json:"organization_id"`
		}
		_ = json.NewDecoder(r.Body).Decode(&body)
		if body.Org != "org-me" {
			t.Errorf("model token asked for organization %q, want the default one", body.Org)
		}
		n := z.minted.Add(1)
		_, _ = io.WriteString(w, `{"token":"llm-`+string(rune('0'+n))+`"}`)
	case "/models":
		if want := "Bearer llm-" + string(rune('0'+z.minted.Load())); r.Header.Get("Authorization") != want {
			t.Errorf("/models signed %q, want %q", r.Header.Get("Authorization"), want)
		}
		_, _ = io.WriteString(w, `{"models":[{"provider":"anthropic","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5","max_token_count":200000},
			{"provider":"open_ai","id":"gpt-5","display_name":"GPT-5"}],"default_model":"claude-sonnet-4-5"}`)
	default:
		t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
		w.WriteHeader(404)
	}
}

func zedStub(t *testing.T, plan string) *zedCloudStub {
	t.Helper()
	z := &zedCloudStub{t: t, plan: plan}
	srv := httptest.NewServer(http.HandlerFunc(z.serve))
	t.Cleanup(srv.Close)
	oldCloud, oldSite := zedCloud, zedSite
	zedCloud, zedSite = srv.URL, "https://zed.test"
	t.Cleanup(func() { zedCloud, zedSite = oldCloud, oldSite })
	zedTokens.m = map[string]string{}
	return z
}

// zedBrowser does what zed.dev's page does once the user signs in: sends
// the browser back to magpie's port with the token encrypted to its key.
func zedBrowser(t *testing.T, signInURL string) *http.Response {
	t.Helper()
	u, err := url.Parse(signInURL)
	if err != nil || u.Host != "zed.test" || u.Path != "/native_app_signin" {
		t.Fatalf("sign-in URL %q", signInURL)
	}
	q := u.Query()
	der, err := base64.URLEncoding.DecodeString(q.Get("native_app_public_key"))
	if err != nil {
		t.Fatal(err)
	}
	pk, err := x509.ParsePKCS1PublicKey(der)
	if err != nil {
		t.Fatal(err)
	}
	ct, err := rsa.EncryptOAEP(sha256.New(), rand.Reader, pk, []byte("plain-access"), nil)
	if err != nil {
		t.Fatal(err)
	}
	back := "http://127.0.0.1:" + q.Get("native_app_port") + "/?user_id=4242&access_token=" + base64.URLEncoding.EncodeToString(ct)
	c := &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	res, err := c.Get(back)
	if err != nil {
		t.Fatal(err)
	}
	res.Body.Close()
	return res
}

func TestZedSignIn(t *testing.T) {
	signIn(t)
	z := zedStub(t, "zed_pro")
	st, err := StartSignIn("zed")
	if err != nil {
		t.Fatal(err)
	}
	u, _ := url.Parse(st.URL)
	z.systemID = u.Query().Get("system_id")
	if z.systemID == "" {
		t.Fatal("the sign-in URL carries no system id")
	}
	res := zedBrowser(t, st.URL)
	if res.StatusCode != http.StatusFound || res.Header.Get("Location") != "https://zed.test/native_app_signin_succeeded" {
		t.Fatalf("the browser was answered %d %q", res.StatusCode, res.Header.Get("Location"))
	}
	done := zedWaitSignIn(t, st.ID)
	if done.State != "done" || done.User != "octo" {
		t.Fatalf("sign-in ended %+v", done)
	}

	ls := Logins("zed")
	if len(ls) != 1 || ls[0].User != "octo" || ls[0].Plan != "Pro" {
		t.Fatalf("logins %+v", ls)
	}
	c, _ := zedLookup("octo")
	if c.Access != "plain-access" || c.UserID != "4242" || c.Org != "org-me" || c.SystemID != z.systemID || len(c.Models) == 0 {
		t.Fatalf("saved %+v", c)
	}
	var p Provider
	for _, a := range Accounts() {
		if a.ID == "zed" {
			p = a
		}
	}
	if p.Account == nil || p.Account.User != "octo" {
		t.Fatalf("no Zed provider among the accounts")
	}
	if ms := p.Account.models(); len(ms) != 2 || ms[0].ID != "claude-sonnet-4-5" {
		t.Fatalf("models %+v", ms)
	}
	if ZedProviderOf("octo", "gpt-5") != "open_ai" || ZedProviderOf("octo", "gemini-9") != "google" {
		t.Fatal("ZedProviderOf")
	}

	// the model token is kept, and minted again only when asked to
	ctx := context.Background()
	a, _ := ZedToken(ctx, "octo", false)
	b, _ := ZedToken(ctx, "octo", false)
	fresh, _ := ZedToken(ctx, "octo", true)
	if a != b || fresh == a {
		t.Fatalf("tokens %q %q %q", a, b, fresh)
	}
	if ms, err := p.Account.fetch(ctx); err != nil || len(ms) != 2 {
		t.Fatalf("fetch: %v %v", ms, err)
	}

	q := zedLoginQuota(ctx, ls[0])
	if q.Error != "" || q.Plan != "Pro" || q.Until == nil || !q.Until.Equal(time.Date(2026, 10, 1, 0, 0, 0, 0, time.UTC)) {
		t.Fatalf("quota %+v", q)
	}

	// Zed refusing the account's pair ends it, and says so
	z.refuse.Store(true)
	if _, err := ZedToken(ctx, "octo", true); !errors.Is(err, ErrZedSignIn) {
		t.Fatalf("a refused sign-in: %v", err)
	}
	if ls := Logins("zed"); len(ls) != 1 || ls[0].Lapsed == "" {
		t.Fatalf("the refused account isn't marked: %+v", ls)
	}
}

func TestZedFreePlanQuota(t *testing.T) {
	signIn(t)
	zedStub(t, "zed_free")
	auth, _ := json.Marshal(zedCreds{UserID: "4242", Access: "plain-access", Org: "org-me"})
	if err := addSideLogin(savedLogin{Agent: "zed", User: "octo", Auth: auth}, "", func(savedLogin) {}); err != nil {
		t.Fatal(err)
	}
	q := zedLoginQuota(context.Background(), Logins("zed")[0])
	if q.Plan != "No plan" || q.Error != "" {
		t.Fatalf("quota %+v", q)
	}
}

func zedWaitSignIn(t *testing.T, id string) SignInState {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	st, err := WaitSignIn(ctx, id)
	if err != nil {
		t.Fatal(err)
	}
	return st
}
