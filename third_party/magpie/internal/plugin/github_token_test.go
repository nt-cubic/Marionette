package plugin

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/ghtoken"
	"github.com/yetone/magpie/internal/settings"
)

// bingqilinweimaotai (#1942): a GitHub token saved in Settings did nothing
// for Check for updates of a plugin installed from GitHub, which kept
// asking GitHub's API without one, 60 an hour for a whole shared address.
// The check asks with the token from Settings, else GITHUB_TOKEN, else
// GH_TOKEN; a used-up rate limit says so, with when it lifts, and a token
// GitHub refuses says which one; raw files on another host never get it.
func TestGitPluginCheckCarriesGitHubToken(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(dir, "cache"))

	var mu sync.Mutex
	var auth []string
	var answer func(w http.ResponseWriter, r *http.Request) bool
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		auth = append(auth, r.Header.Get("Authorization"))
		a := answer
		mu.Unlock()
		if r.URL.Path != "/repos/me/opencode-x-auth/commits" {
			http.NotFound(w, r)
			return
		}
		if a != nil && a(w, r) {
			return
		}
		fmt.Fprint(w, `[{"sha":"abc1234def5678abc1234def5678abc1234def56"}]`)
	}))
	t.Cleanup(api.Close)
	was := githubAPI
	githubAPI = api.URL
	t.Cleanup(func() { githubAPI = was })
	lastAuth := func() string {
		mu.Lock()
		defer mu.Unlock()
		if len(auth) == 0 {
			return "(not asked)"
		}
		return auth[len(auth)-1]
	}
	setToken := func(tok string) {
		s := settings.Load()
		s.GitHubToken = tok
		if err := settings.Save(s); err != nil {
			t.Fatal(err)
		}
	}

	// a plugin bun installed from github:me/opencode-x-auth at abc1234
	const spec = "github:me/opencode-x-auth"
	if err := save(List{Plugins: []Entry{{Spec: spec}}}); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(Dir(), "package.json"), []byte(`{"dependencies":{"opencode-x-auth":"`+spec+`"}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	installedAt(t, "opencode-x-auth", "0.1.0")
	if err := os.WriteFile(filepath.Join(Target(spec), ".bun-tag"), []byte("me-opencode-x-auth-abc1234\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	check := func() VersionCheck {
		t.Helper()
		cs := CheckNow(context.Background()).Plugins
		if len(cs) != 1 {
			t.Fatalf("checked %+v", cs)
		}
		return cs[0]
	}

	for _, c := range []struct{ name, setting, github, gh, want string }{
		{"none", "", "", "", ""},
		{"GH_TOKEN", "", "", "gh-env", "Bearer gh-env"},
		{"GITHUB_TOKEN", "", "github-env", "gh-env", "Bearer github-env"},
		{"settings", "ghp_settings", "github-env", "gh-env", "Bearer ghp_settings"},
	} {
		t.Setenv("GITHUB_TOKEN", c.github)
		t.Setenv("GH_TOKEN", c.gh)
		setToken(c.setting)
		if got := check(); got.Status != "current" {
			t.Errorf("%s: %+v", c.name, got)
		}
		if a := lastAuth(); a != c.want {
			t.Errorf("%s: GitHub's API was asked with Authorization %q, want %q", c.name, a, c.want)
		}
	}

	// the rate limit used up: said, with when it lifts, the token never
	reset := time.Date(2030, 1, 2, 3, 4, 0, 0, time.UTC)
	mu.Lock()
	answer = func(w http.ResponseWriter, r *http.Request) bool {
		w.Header().Set("X-RateLimit-Remaining", "0")
		w.Header().Set("X-RateLimit-Reset", fmt.Sprint(reset.Unix()))
		w.WriteHeader(403)
		fmt.Fprint(w, `{"message":"API rate limit exceeded for 1.2.3.4."}`)
		return true
	}
	mu.Unlock()
	c := check()
	if c.Status != "unknown" || c.Why != "limited" || c.Limited == nil || !c.Limited.Token || c.Limited.Until != "2030-01-02T03:04:00Z" {
		t.Fatalf("limited, with a token: %+v %+v", c, c.Limited)
	}
	if !strings.Contains(c.Error, "rate limit for your GitHub token") || strings.Contains(c.Error, "ghp_settings") {
		t.Errorf("limited, with a token: %q", c.Error)
	}
	setToken("")
	t.Setenv("GITHUB_TOKEN", "")
	t.Setenv("GH_TOKEN", "")
	if c = check(); c.Limited == nil || c.Limited.Token || !strings.Contains(c.Error, "Add a GitHub token in Settings") {
		t.Errorf("limited, without a token: %+v %+v", c, c.Limited)
	}
	// a secondary limit: 429 with Retry-After
	mu.Lock()
	answer = func(w http.ResponseWriter, r *http.Request) bool {
		w.Header().Set("Retry-After", "60")
		w.WriteHeader(429)
		return true
	}
	mu.Unlock()
	if c = check(); c.Why != "limited" || c.Limited == nil || c.Limited.Until == "" {
		t.Errorf("secondary limit: %+v %+v", c, c.Limited)
	}

	// a token GitHub refuses (401): the check goes on without it rather
	// than failing whole, and the anonymous answer is the one taken. The
	// Settings says which token to replace (#3926).
	mu.Lock()
	answer = func(w http.ResponseWriter, r *http.Request) bool {
		if r.Header.Get("Authorization") == "" {
			return false
		}
		w.WriteHeader(401)
		fmt.Fprint(w, `{"message":"Bad credentials"}`)
		return true
	}
	before := len(auth)
	mu.Unlock()
	setToken("ghp_settings_refused")
	if c = check(); c.Limited != nil || c.Error != "" || c.Status != "current" {
		t.Errorf("Settings' token refused: %+v", c)
	}
	mu.Lock()
	if a := auth[before:]; len(a) != 2 || a[0] != "Bearer ghp_settings_refused" || a[1] != "" {
		t.Errorf("Settings' token refused: GitHub was asked with %q, want the token then none", a)
	}
	mu.Unlock()
	if _, ok := ghtoken.SettingsRefused(); !ok {
		t.Error("ghtoken.SettingsRefused() says no token of Settings was refused")
	}
	setToken("")
	// one from the environment (till on Discord: a stale GITHUB_TOKEN) is
	// dropped, and the check asked again without it
	t.Setenv("GH_TOKEN", "bad-token")
	mu.Lock()
	n := len(auth)
	mu.Unlock()
	if c = check(); c.Status != "current" || c.Error != "" {
		t.Errorf("GH_TOKEN refused: %+v", c)
	}
	mu.Lock()
	if a := auth[n:]; len(a) != 2 || a[0] != "Bearer bad-token" || a[1] != "" {
		t.Errorf("GH_TOKEN refused: GitHub was asked with %q, want the token then none", a)
	}
	n = len(auth)
	mu.Unlock()
	if c = check(); c.Status != "current" {
		t.Errorf("GH_TOKEN refused, checked again: %+v", c)
	}
	mu.Lock()
	if a := auth[n:]; len(a) != 1 || a[0] != "" {
		t.Errorf("GH_TOKEN refused, checked again: GitHub was asked with %q, want none", a)
	}
	mu.Unlock()

	// another host (raw files) never gets the token
	var rawAuth string
	raw := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		rawAuth = r.Header.Get("Authorization")
		fmt.Fprint(w, `{}`)
	}))
	t.Cleanup(raw.Close)
	if _, err := fetchJSON(context.Background(), raw.URL+"/me/opencode-x-auth/HEAD/package.json", 1<<10); err != nil {
		t.Fatal(err)
	}
	if rawAuth != "" {
		t.Errorf("a host other than GitHub's API was sent Authorization %q", rawAuth)
	}
}

// Bun's newest release, asked of GitHub's API, carries the token too; a
// Bun download from another host never does (#1942).
func TestBunReleaseCheckCarriesGitHubToken(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	t.Setenv("USERPROFILE", dir)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(dir, "config"))
	t.Setenv("GITHUB_TOKEN", "github-env")
	var apiAuth, dlAuth string
	api := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		apiAuth = r.Header.Get("Authorization")
		fmt.Fprint(w, `{"tag_name":"bun-v1.4.2","published_at":"2026-10-01T00:00:00Z"}`)
	}))
	t.Cleanup(api.Close)
	dl := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		dlAuth = r.Header.Get("Authorization")
		fmt.Fprint(w, "zip")
	}))
	t.Cleanup(dl.Close)
	was := githubAPI
	githubAPI = api.URL
	t.Cleanup(func() { githubAPI = was })
	if v, _, err := bunLatest(context.Background()); err != nil || v != "1.4.2" {
		t.Fatalf("bunLatest = %q, %v", v, err)
	}
	if apiAuth != "Bearer github-env" {
		t.Errorf("Bun's release was asked of GitHub's API with Authorization %q", apiAuth)
	}
	if _, err := getURL(context.Background(), dl.URL+"/bun-darwin-aarch64.zip", 1<<10); err != nil {
		t.Fatal(err)
	}
	if dlAuth != "" {
		t.Errorf("a download from another host was sent Authorization %q", dlAuth)
	}
}
