package provider

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/testenv"
)

// setProxies gives subscription id a proxy of its own, and its accounts
// theirs, as the editor saves them.
func setProxies(t *testing.T, id, proxy string, own map[string]string) {
	t.Helper()
	f, err := read()
	if err != nil {
		t.Fatal(err)
	}
	f.Providers = slices.DeleteFunc(f.Providers, func(p Provider) bool { return p.ID == id })
	f.Providers = append(f.Providers, Provider{ID: id, Proxy: proxy, AccountProxies: own})
	if err := store(f); err != nil {
		t.Fatal(err)
	}
}

// codexVendorProxy is a proxy that answers Codex's token exchange itself,
// as OpenAI would, the code naming the account; it tells what it was
// asked.
func codexVendorProxy(t *testing.T) (*httptest.Server, func() []string) {
	t.Helper()
	var mu sync.Mutex
	var seen []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r.URL.String())
		mu.Unlock()
		r.ParseForm()
		user := r.Form.Get("code") + "@example.com"
		json.NewEncoder(w).Encode(map[string]any{
			"id_token": fakeJWT(map[string]any{"email": user,
				"https://api.openai.com/auth": map[string]any{"chatgpt_plan_type": "plus", "chatgpt_account_id": "acct-" + user}}),
			"access_token":  fakeJWT(map[string]any{"exp": float64(time.Now().Add(time.Hour).Unix())}),
			"refresh_token": "r-" + user,
		})
	}))
	t.Cleanup(srv.Close)
	return srv, func() []string {
		mu.Lock()
		defer mu.Unlock()
		return slices.Clone(seen)
	}
}

// #1663 (demystify0137): an account is signed in through the proxy its
// requests will go through, the subscription's, not the global one, so the
// vendor sees it come from one place. A proxy picked for the sign-in is
// kept as the account's: the subscription's for its only account, else
// that account's own.
func TestSignInGoesThroughItsProxy(t *testing.T) {
	claudeHome(t)
	dialed := dialsOut(t)
	sub, subSaw := codexVendorProxy(t)
	other, otherSaw := codexVendorProxy(t)
	oldTok, oldAddr := codexTokenURL, codexCallbackAddr
	t.Cleanup(func() { codexTokenURL, codexCallbackAddr = oldTok, oldAddr })
	codexTokenURL = "http://auth.openai.invalid/oauth/token"
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	codexCallbackAddr = ln.Addr().String()
	ln.Close()
	const token = "http://auth.openai.invalid/oauth/token"

	signIn := func(o SignInOptions, code string) SignInState {
		t.Helper()
		st, err := StartSignInWith("codex", o)
		if err != nil {
			t.Fatal(err)
		}
		finishInBrowser(t, st, code)
		if st = waitDone(t, st.ID); st.State != "done" || st.User != code+"@example.com" || st.Error != "" {
			t.Fatalf("state %+v", st)
		}
		time.Sleep(1200 * time.Millisecond) // the callback port is let go
		return st
	}
	stored := func() Provider {
		t.Helper()
		p, _ := storedPicks("codex")
		return p
	}

	// the first account, through a proxy picked for it: the subscription's
	// from now on, as its only account's proxy is
	pick := sub.URL
	if st := signIn(SignInOptions{Proxy: &pick}, "a"); st.Proxy != sub.URL {
		t.Fatalf("the sign-in says it goes through %q", st.Proxy)
	}
	if got := subSaw(); !slices.Equal(got, []string{token}) {
		t.Fatalf("the picked proxy saw %q", got)
	}
	if p := stored(); p.Proxy != sub.URL || len(p.AccountProxies) != 0 {
		t.Fatalf("kept as proxy %q, accounts' %v", p.Proxy, p.AccountProxies)
	}

	// the next, picked nothing: through the subscription's, where it went
	// through the global one
	if st := signIn(SignInOptions{}, "b"); st.Proxy != sub.URL {
		t.Fatalf("the sign-in says it goes through %q", st.Proxy)
	}
	if got := subSaw(); !slices.Equal(got, []string{token, token}) {
		t.Fatalf("the subscription's proxy saw %q", got)
	}

	// one more, through another: that account's own, the others' left
	pick = other.URL
	signIn(SignInOptions{Proxy: &pick}, "c")
	if got := otherSaw(); !slices.Equal(got, []string{token}) {
		t.Fatalf("the picked proxy saw %q", got)
	}
	if p := stored(); p.Proxy != sub.URL || len(p.AccountProxies) != 1 || p.AccountProxies["c@example.com"] != other.URL {
		t.Fatalf("kept as proxy %q, accounts' %v", p.Proxy, p.AccountProxies)
	}
	if got := ProxyOfLogin("codex", "C@example.com"); got != other.URL {
		t.Fatalf("c's requests go through %q", got)
	}

	// signing in to it again goes through its own
	st, err := StartSignInWith("codex", SignInOptions{User: "c@example.com"})
	if err != nil {
		t.Fatal(err)
	}
	CancelSignIn(st.ID)
	if st.Proxy != other.URL {
		t.Fatalf("signing c in again goes through %q", st.Proxy)
	}
	if d := dialed(); len(d) != 0 {
		t.Fatalf("dialed %v past the proxies", d)
	}

	// a proxy that isn't one is refused before anything starts
	bad := "ftp://x"
	if _, err := StartSignInWith("codex", SignInOptions{Proxy: &bad}); err == nil {
		t.Fatal("ftp://x was taken as a proxy")
	}
}

// A device sign-in run by magpie (Copilot's: the code asked for, then
// polled for the token, then whose account it is) goes through the
// subscription's proxy from its first request (#1663).
func TestDeviceSignInGoesThroughItsProxy(t *testing.T) {
	signIn(t)
	dialed := dialsOut(t)
	var mu sync.Mutex
	var seen []string
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r.URL.Path)
		mu.Unlock()
		switch r.URL.Path {
		case "/login/device/code":
			json.NewEncoder(w).Encode(map[string]any{"device_code": "dc", "user_code": "ABCD-1234", "verification_uri": "https://github.com/login/device", "interval": 0})
		case "/login/oauth/access_token":
			json.NewEncoder(w).Encode(map[string]any{"access_token": "gho_p"})
		case "/user":
			json.NewEncoder(w).Encode(map[string]any{"login": "proxied"})
		case "/copilot_internal/user":
			json.NewEncoder(w).Encode(map[string]any{"copilot_plan": "individual"})
		default:
			w.WriteHeader(404)
		}
	}))
	t.Cleanup(proxy.Close)
	const gh = "http://github.invalid"
	oldDev, oldTok, oldCU, oldGU := gitHubDeviceURL, gitHubTokenURL, CopilotUserURL, GitHubUserURL
	gitHubDeviceURL, gitHubTokenURL = gh+"/login/device/code", gh+"/login/oauth/access_token"
	CopilotUserURL, GitHubUserURL = gh+"/copilot_internal/user", gh+"/user"
	t.Cleanup(func() { gitHubDeviceURL, gitHubTokenURL, CopilotUserURL, GitHubUserURL = oldDev, oldTok, oldCU, oldGU })
	setProxies(t, "copilot", proxy.URL, nil)

	st, err := StartSignIn("copilot")
	if err != nil {
		t.Fatal(err)
	}
	if st = waitDone(t, st.ID); st.State != "done" || st.User != "proxied" {
		t.Fatalf("state %+v", st)
	}
	mu.Lock()
	got := slices.Clone(seen)
	mu.Unlock()
	if len(got) < 3 || got[0] != "/login/device/code" || !slices.Contains(got, "/login/oauth/access_token") || !slices.Contains(got, "/user") {
		t.Fatalf("the subscription's proxy saw %q", got)
	}
	if d := dialed(); len(d) != 0 {
		t.Fatalf("dialed %v past the proxy", d)
	}
}

// A plugin's sign-in goes through its provider's proxy too: the plugin's
// requests while it authorizes, and when it trades the code pasted back
// (#1663).
func TestPluginSignInGoesThroughItsProxy(t *testing.T) {
	bun := testenv.Bun(t)
	claudeHome(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Setenv("FAKE_SIGNIN", "http://vendor.invalid/signin")
	t.Cleanup(plugin.Settle)
	var mu sync.Mutex
	var seen []string
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r.URL.String())
		mu.Unlock()
	}))
	t.Cleanup(proxy.Close)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	setProxies(t, "fakeco", proxy.URL, nil)
	st, err := StartPluginSignIn("fakeco", 1, map[string]string{"where": "home"})
	if err != nil || st.Proxy != proxy.URL {
		t.Fatalf("StartPluginSignIn = %+v, %v", st, err)
	}
	if err := SubmitSignInCallback(st.ID, "good"); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	if !slices.Equal(seen, []string{"http://vendor.invalid/signin/device", "http://vendor.invalid/signin/token"}) {
		t.Fatalf("the provider's proxy saw %q", seen)
	}
}

// Claude Code's own sign-in is run with the subscription's proxy, where it
// was run with the global one (#1663).
func TestClaudeSignInGoesThroughItsProxy(t *testing.T) {
	claudeHome(t)
	noAnthropic(t)
	exe := fakeClaudeLogin(t, fakeClaudeAccount{email: "p@example.com", name: "P", plan: "max", refresh: "sk-ant-ort01-p"}, "", true)
	setProxies(t, "claude", "http://127.0.0.1:9", nil)
	st, err := StartSignIn("claude")
	if err != nil {
		t.Fatal(err)
	}
	finishInBrowser(t, st, "the-code")
	if st = waitDone(t, st.ID); st.State != "done" {
		t.Fatalf("state %+v", st)
	}
	b, _ := os.ReadFile(filepath.Join(filepath.Dir(exe), "login-proxy"))
	if strings.TrimRight(string(b), "/") != "http://127.0.0.1:9" {
		t.Fatalf("claude auth login ran with HTTPS_PROXY=%q", b)
	}
}

// Signing out of a ChatGPT account, OpenAI revokes its refresh token
// through the account's own proxy, the exit its sign-in and requests left
// through, not the global one (#1663).
func TestChatGPTSignOutGoesThroughItsProxy(t *testing.T) {
	claudeHome(t)
	f := newFakeOpenAI(t)
	if done := signInVia(t, f, "oaiapp_123", same); done.State != "done" || done.User != "me@example.com" {
		t.Fatalf("sign-in ended %+v", done)
	}
	dialed := dialsOut(t)
	var mu sync.Mutex
	var seen []string
	record := func(who string) *httptest.Server {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			mu.Lock()
			seen = append(seen, who+" "+r.URL.String())
			mu.Unlock()
			if r.URL.Path == "/.well-known/openid-configuration" {
				json.NewEncoder(w).Encode(map[string]any{"revocation_endpoint": "http://openai.invalid/oauth/revoke"})
			}
		}))
		t.Cleanup(srv.Close)
		return srv
	}
	sub, own := record("sub"), record("own")
	setProxies(t, ChatGPTAPIID, sub.URL, map[string]string{"me@example.com": own.URL})
	oldI := siwcIssuer
	siwcIssuer = "http://openai.invalid"
	t.Cleanup(func() { siwcIssuer = oldI })

	if err := ForgetLogin(ChatGPTAPIID, "me@example.com"); err != nil {
		t.Fatal(err)
	}
	want := []string{"own http://openai.invalid/.well-known/openid-configuration", "own http://openai.invalid/oauth/revoke"}
	for deadline := time.Now().Add(10 * time.Second); ; time.Sleep(20 * time.Millisecond) {
		mu.Lock()
		got := slices.Clone(seen)
		mu.Unlock()
		if slices.Equal(got, want) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("the sign-out went through %q, dialed %v", got, dialed())
		}
	}
	if d := dialed(); len(d) != 0 {
		t.Fatalf("dialed %v past the proxy", d)
	}
}

// Devin's CLI, asked who is signed in and for its models, runs with the
// account's own proxy, and while a sign-in asks it, with the proxy the
// sign-in goes through, where it always had the global one (#1663).
func TestDevinCLIGoesThroughItsProxy(t *testing.T) {
	shellFakes(t)
	home := claudeHome(t)
	data := filepath.Join(home, "data")
	t.Setenv("XDG_DATA_HOME", data)
	os.MkdirAll(filepath.Join(data, "devin"), 0o700)
	own := string(devinCredentials("devin-session-token$own", "", "", ""))
	os.WriteFile(filepath.Join(data, "devin", "credentials.toml"), []byte(own), 0o600)
	// a CLI that notes the proxy it was run with, and says whose key is in
	// the credentials.toml of its data folder
	exe := filepath.Join(home, "devin")
	runs := filepath.Join(home, "runs")
	testenv.Program(t, exe, `#!/bin/sh
f="$XDG_DATA_HOME/devin/credentials.toml"
if grep -q own "$f" 2>/dev/null; then who=dev@example.com; else who=two@example.com; fi
echo "$who $1 $HTTPS_PROXY" >> `+runs+`
case "$1" in
models) echo '[]'; exit 0;;
esac
printf 'Logged in (via Devin).

User:
  Email:             %s

Account:
  Tier:              Devin Pro
' "$who"
`)
	fakeDevin(t, exe)
	exchange := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]any{"sessionToken": "devin-session-token$two"})
	}))
	t.Cleanup(exchange.Close)
	oldTok := devinExchangeURL
	devinExchangeURL = exchange.URL
	t.Cleanup(func() { devinExchangeURL = oldTok })
	const sub, mine, picked = "http://127.0.0.1:7001", "http://127.0.0.1:7002", "http://127.0.0.1:7003"
	setProxies(t, "devin", sub, map[string]string{"dev@example.com": mine})
	ran := func() []string {
		b, _ := os.ReadFile(runs)
		os.Remove(runs)
		var out []string
		for _, l := range strings.Split(strings.TrimSpace(string(b)), "\n") {
			if l != "" {
				out = append(out, strings.TrimRight(l, "/"))
			}
		}
		return out
	}
	forgetDevinStatus()
	ran()

	// a second account, signed in through a proxy picked for it: the CLI
	// asked whose it is runs with that proxy
	p := picked
	st, err := StartSignInWith("devin", SignInOptions{Proxy: &p})
	if err != nil {
		t.Fatal(err)
	}
	finishInBrowser(t, st, "dv-code")
	if st = waitDone(t, st.ID); st.State != "done" || st.User != "two@example.com" {
		t.Fatalf("state %+v", st)
	}
	if got := ran(); !slices.Contains(got, "two@example.com auth "+picked) ||
		slices.ContainsFunc(got, func(l string) bool { return strings.HasPrefix(l, "two@") && !strings.HasSuffix(l, " "+picked) }) {
		t.Fatalf("the sign-in ran the CLI as %q", got)
	}

	// later, each account's CLI runs with that account's own proxy
	var homeOf = map[string]string{}
	for _, l := range devinLogins() {
		homeOf[l.User] = l.Home
	}
	two, ok := homeOf["two@example.com"]
	if !ok || two == "" {
		t.Fatalf("logins %v", homeOf)
	}
	ran()
	if _, err := askDevinFamiliesAt(context.Background(), two); err != nil && !strings.Contains(err.Error(), "no models") {
		t.Logf("models: %v", err)
	}
	askDevinStatusAt(context.Background(), two)
	askDevinStatusAt(context.Background(), "")
	got := ran()
	for _, want := range []string{"two@example.com models " + picked, "two@example.com auth " + picked, "dev@example.com auth " + mine} {
		if !slices.Contains(got, want) {
			t.Fatalf("ran %q, no %q", got, want)
		}
	}
}
