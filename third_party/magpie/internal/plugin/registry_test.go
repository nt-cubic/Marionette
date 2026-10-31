package plugin

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/source"
	"github.com/yetone/magpie/internal/testenv"
)

// registryHome is a home of its own with no registry anywhere: none in
// the environment, the login shell's, a bunfig or an .npmrc, the 「国内镜像」
// switch off and nothing set in magpie.
func registryHome(t *testing.T) (home string, shell map[string]string) {
	t.Helper()
	home = t.TempDir()
	testenv.SetHome(t, home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("APPDATA", filepath.Join(home, "AppData"))
	for _, k := range registryVars {
		t.Setenv(k, "")
	}
	shell = map[string]string{}
	le, china := loginEnv, source.China
	loginEnv = func(k string) string { return shell[k] }
	source.China = func() bool { return false }
	t.Cleanup(func() { loginEnv, source.China = le, china })
	return home, shell
}

func writeFile(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
}

// Each place a registry can be named, each before the ones after it:
// magpie's own setting, the environment, the login shell, a bunfig, an
// .npmrc, the 「国内镜像」 switch, npm's own (timwangdev on X). The switch
// never comes before a registry the user named.
func TestRegistryPrecedence(t *testing.T) {
	home, shell := registryHome(t)
	want := func(url, from string) {
		t.Helper()
		if r := Registry(); r.URL != url || r.From != from {
			t.Fatalf("Registry() = %+v, want %s from %s", r, url, from)
		}
	}
	want("https://registry.npmjs.org", FromDefault)
	source.China = func() bool { return true }
	want("https://registry.npmmirror.com", FromChina)
	writeFile(t, filepath.Join(home, ".npmrc"), "# mine\nregistry = \"https://npm.corp.example/repo/\"\n")
	want("https://npm.corp.example/repo", FromNPMRC)
	writeFile(t, filepath.Join(home, ".bunfig.toml"), "[install]\nregistry = { url = \"https://bun.corp.example/\", token = \"x\" }\n")
	want("https://bun.corp.example", FromBunfig)
	shell["NPM_CONFIG_REGISTRY"] = "https://shell.example/"
	want("https://shell.example", FromShell)
	t.Setenv("BUN_CONFIG_REGISTRY", "https://env.example")
	want("https://env.example", FromEnv)
	if _, err := SetRegistry("https://magpie.example/npm/"); err != nil {
		t.Fatal(err)
	}
	want("https://magpie.example/npm", FromMagpie)
	if _, err := SetRegistry("default"); err != nil {
		t.Fatal(err)
	}
	want("https://env.example", FromEnv)
}

// An address that isn't an http(s) registry's is refused and not kept;
// the one set before stays.
func TestSetRegistryRefusesWhatIsntOne(t *testing.T) {
	registryHome(t)
	if _, err := SetRegistry("https://ok.example/"); err != nil {
		t.Fatal(err)
	}
	for _, bad := range []string{"ftp://npm.example", "npm.example", "javascript:alert(1)", "file:///etc/passwd", "https://", "--help", "https://user:pw@npm.example", "https://npm.example/?x=1"} {
		if _, err := SetRegistry(bad); err == nil {
			t.Errorf("SetRegistry(%q) was taken", bad)
		}
		if got := settings.Load().NPMRegistry; got != "https://ok.example" {
			t.Fatalf("after %q settings say %q", bad, got)
		}
	}
	// one written into settings.json by hand that isn't one is passed over
	s := settings.Load()
	s.NPMRegistry = "ftp://npm.example"
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if r := Registry(); r.From != FromDefault {
		t.Fatalf("a bad setting is used: %+v", r)
	}
}

// bun is told the registry where it wouldn't find it itself: magpie's
// setting, over the environment's; the login shell's; npmmirror with the
// switch on. A registry in the user's .npmrc or bunfig is bun's to read,
// and the switch's mirror is never put over it.
func TestBunTakesTheUsersRegistry(t *testing.T) {
	home, shell := registryHome(t)
	reg := func() string {
		var got string
		for _, kv := range bunCommand(context.Background(), "bun", t.TempDir(), "add", "x").Env {
			if v, ok := strings.CutPrefix(kv, "BUN_CONFIG_REGISTRY="); ok {
				got = v // the last is the one bun sees
			}
		}
		return got
	}
	source.China = func() bool { return true }
	if r := reg(); r != "https://registry.npmmirror.com/" {
		t.Fatalf("switch on: %q", r)
	}
	writeFile(t, filepath.Join(home, ".npmrc"), "registry=https://npm.corp.example/\n")
	if r := reg(); r != "" {
		t.Fatalf("with the user's .npmrc, bun is told %q", r)
	}
	shell["BUN_CONFIG_REGISTRY"] = "https://shell.example/"
	if r := reg(); r != "https://shell.example/" {
		t.Fatalf("the login shell's: %q", r)
	}
	t.Setenv("NPM_CONFIG_REGISTRY", "https://env.example/")
	if r := reg(); r != "" {
		t.Fatalf("the environment's own is bun's to read: %q", r)
	}
	if _, err := SetRegistry("https://magpie.example"); err != nil {
		t.Fatal(err)
	}
	if r := reg(); r != "https://magpie.example/" {
		t.Fatalf("magpie's setting: %q", r)
	}
}

// magpie asks npm's questions (a package's newest version, its README,
// the search) of the user's registry, with the token their .npmrc keeps
// for that host and for no other; a scope's own registry is asked of its
// packages. npm's own is never asked, with the switch on or off.
func TestLookupsAskTheUsersRegistry(t *testing.T) {
	home, _ := registryHome(t)
	source.China = func() bool { return true }
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	var mu sync.Mutex
	var seen []string
	serve := func(name string) *httptest.Server {
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			mu.Lock()
			seen = append(seen, name+" "+r.URL.Path+" "+r.Header.Get("Authorization"))
			mu.Unlock()
			switch {
			case strings.HasSuffix(r.URL.Path, "/latest"):
				w.Write([]byte(`{"version":"1.2.3","description":"from ` + name + `"}`))
			case strings.HasPrefix(r.URL.Path, "/-/v1/search"):
				w.Write([]byte(`{"objects":[]}`))
			default:
				w.Write([]byte(`{"readme":"readme from ` + name + `"}`))
			}
		}))
		t.Cleanup(srv.Close)
		return srv
	}
	mine, scoped, npm := serve("mine"), serve("scoped"), serve("npmjs")
	// npm's download counts are npm's alone, whatever the registry
	counts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte(`{"downloads":1}`)) }))
	t.Cleanup(counts.Close)
	was, wasDL := npmRegistry, npmDownloads
	npmRegistry, npmDownloads = npm.URL, counts.URL
	t.Cleanup(func() { npmRegistry, npmDownloads = was, wasDL })
	t.Setenv("MY_TOKEN", "s3cret")
	writeFile(t, filepath.Join(home, ".npmrc"), strings.Join([]string{
		"registry=" + mine.URL + "/",
		"@corp:registry=" + scoped.URL,
		"//" + strings.TrimPrefix(mine.URL, "http://") + "/:_authToken=${MY_TOKEN}",
	}, "\n"))
	ctx := context.Background()

	info, err := npmAsk(ctx, "opencode-x-auth")
	if err != nil || info.Version != "1.2.3" {
		t.Fatalf("npmAsk: %+v %v", info, err)
	}
	if p, err := Readme(ctx, "opencode-x-auth"); err != nil || !strings.Contains(p.Readme, "from mine") {
		t.Fatalf("Readme: %+v %v", p, err)
	}
	if _, err := npmAsk(ctx, "@corp/opencode-y-auth"); err != nil {
		t.Fatal(err)
	}
	if _, err := searchNPM(ctx, "auth", func(string, []string) bool { return true }); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	defer mu.Unlock()
	for _, s := range seen {
		switch {
		case strings.HasPrefix(s, "npmjs "):
			t.Errorf("npm's own was asked: %s", s)
		case strings.HasPrefix(s, "mine ") && !strings.HasSuffix(s, " Bearer s3cret"):
			t.Errorf("the user's registry was asked without their token: %s", s)
		case strings.HasPrefix(s, "scoped ") && strings.Contains(s, "Bearer"):
			t.Errorf("another host's token went to the scope's registry: %s", s)
		}
	}
	for _, w := range []string{"mine /opencode-x-auth/latest Bearer s3cret", "mine /opencode-x-auth Bearer s3cret", "scoped /@corp/opencode-y-auth/latest ", "mine /-/v1/search Bearer s3cret"} {
		if !slices.ContainsFunc(seen, func(s string) bool { return strings.EqualFold(s, w) }) {
			t.Errorf("not asked: %q (asked %q)", w, seen)
		}
	}
}
