package gui

import (
	"io/fs"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/settings"
)

// onePortHome is a fresh magpie home for one test: settings and keys of
// its own.
func onePortHome(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	t.Setenv("MAGPIE_ADDR", "")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	t.Setenv("MAGPIE_TRUST_PROXY", "")
}

// onePortKey shares magpie and makes an enabled gateway key.
func onePortKey(t *testing.T) string {
	t.Helper()
	s := settings.Load()
	s.LAN = true
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	k, err := access.Update("add-key", access.Change{Name: "nginx"})
	if err != nil {
		t.Fatal(err)
	}
	return k
}

// onePortCall is a request to h from peer, with hdr's pairs, the page's
// cookie added when cookie isn't "".
func onePortCall(t *testing.T, h http.Handler, method, target, peer, host, cookie string, hdr ...string) *httptest.ResponseRecorder {
	t.Helper()
	var body *strings.Reader
	if method == "POST" {
		body = strings.NewReader(`{"model":"none/none","messages":[{"role":"user","content":"hi"}]}`)
	} else {
		body = strings.NewReader("")
	}
	r := httptest.NewRequest(method, target, body)
	r.RemoteAddr = peer
	r.Host = host
	r.Header.Set("Content-Type", "text/plain") // a page's "simple" POST
	for i := 0; i+1 < len(hdr); i += 2 {
		r.Header.Set(hdr[i], hdr[i+1])
	}
	if cookie != "" {
		r.AddCookie(&http.Cookie{Name: "magpie_web_3430", Value: cookie})
	}
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

var chromeMarks = []string{"User-Agent", "Mozilla/5.0 (Macintosh) Chrome/140", "Sec-Fetch-Site", "same-origin", "Sec-Fetch-Mode", "cors"}

// onePortCases checks what `magpie web`'s port answers, the gateway being
// reached through gw (this process's, or another magpie's): its client
// routes as the gateway's own port would, and the page's API never without
// the page's key, whatever route, Origin or Host a request takes.
func onePortCases(t *testing.T, h http.Handler, reached *bool) {
	t.Helper()
	const lo, here = "127.0.0.1:50001", "127.0.0.1:3430"
	const evil = "rebind.attacker.example:3430"
	api := func(name, method, target, peer, host string, hdr ...string) {
		t.Helper()
		*reached = false
		w := onePortCall(t, h, method, target, peer, host, "", hdr...)
		if *reached || w.Code < 300 || w.Code == http.StatusMovedPermanently {
			t.Errorf("the page's API %s: %d, reached %v: %s", name, w.Code, *reached, w.Body.String())
		}
	}
	// the page's API: no cookie, no way in
	api("with no Origin", "GET", "/api/settings", lo, here)
	api("with no Origin, a POST", "POST", "/api/settings", lo, here)
	api("from Origin: null", "POST", "/api/settings", lo, here, "Origin", "null")
	api("from a foreign Origin", "POST", "/api/settings", lo, here, "Origin", "https://evil.example")
	api("under a rebound Host", "GET", "/api/settings", lo, evil, chromeMarks...)
	api("under a rebound Host, a POST", "POST", "/api/settings", lo, evil, append([]string{"Origin", "http://" + evil}, chromeMarks...)...)
	api("through a proxy", "GET", "/api/settings", lo, "magpie.example.com", "X-Forwarded-For", "203.0.113.9")
	api("from another machine", "GET", "/api/settings", "10.0.0.5:50001", here)
	// nor by a path that names a client route on its way to the API
	api("by /v1/../api", "GET", "/v1/../api/settings", lo, here)
	api("by /v1/%2e%2e/api", "GET", "/v1/%2e%2e/api/settings", lo, here)
	api("by /v1//api", "GET", "/v1//api/settings", lo, here)
	// a gateway key is no key to the page
	key := onePortKey(t)
	api("with a gateway key", "GET", "/api/settings", lo, here, "Authorization", "Bearer "+key)

	*reached = false
	if w := onePortCall(t, h, "POST", "/api/settings", lo, here, onePortWebKey); w.Code != http.StatusTeapot || !*reached {
		t.Fatalf("the page's API with its cookie: %d", w.Code)
	}

	gw := func(name string, want int, method, target, peer, host, cookie string, hdr ...string) {
		t.Helper()
		*reached = false
		w := onePortCall(t, h, method, target, peer, host, cookie, hdr...)
		if w.Code != want || *reached {
			t.Errorf("the gateway %s: %d, want %d (page reached %v): %s", name, w.Code, want, *reached, w.Body.String())
		}
	}
	// the gateway's client routes, judged as on its own port
	gw("for an agent here", 200, "GET", "/v1/models", lo, here, "")
	gw("from Origin: null", 403, "POST", "/v1/chat/completions", lo, here, "", "Origin", "null")
	gw("from a foreign Origin", 403, "POST", "/v1/chat/completions", lo, here, "", "Origin", "https://evil.example")
	gw("under a rebound Host", 403, "GET", "/v1/models", lo, evil, "", chromeMarks...)
	gw("under a rebound Host, a POST", 403, "POST", "/v1/chat/completions", lo, evil, "", append([]string{"Origin", "http://" + evil}, chromeMarks...)...)
	// the page's own cookie is no gateway key either
	gw("under a rebound Host with the page's cookie", 403, "GET", "/v1/models", lo, evil, onePortWebKey, chromeMarks...)
	gw("through a proxy without a key", 401, "GET", "/v1/models", lo, "magpie.example.com", "", "X-Forwarded-For", "203.0.113.9")
	gw("through a proxy with the page's cookie", 401, "GET", "/v1/models", lo, "magpie.example.com", onePortWebKey, "X-Forwarded-For", "203.0.113.9")
	gw("from another machine without a key", 401, "GET", "/v1/models", "10.0.0.5:50001", here, "")
	gw("through a proxy with a gateway key", 200, "GET", "/v1/models", lo, "magpie.example.com", "", "X-Forwarded-For", "203.0.113.9", "Authorization", "Bearer "+key)
	gw("from another machine with a gateway key", 200, "GET", "/v1/models", "10.0.0.5:50001", here, "", "Authorization", "Bearer "+key)

	// not shared: another machine, or a proxy, isn't answered at all
	s := settings.Load()
	s.LAN = false
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	gw("through a proxy, not shared", 403, "GET", "/v1/models", lo, "magpie.example.com", "", "X-Forwarded-For", "203.0.113.9", "Authorization", "Bearer "+key)
	gw("from another machine, not shared", 403, "GET", "/v1/models", "10.0.0.5:50001", here, "", "Authorization", "Bearer "+key)
}

const onePortWebKey = "0123456789abcdef0123456789abcdef"

func onePortPage(reached *bool) http.Handler {
	return webServed("3430", onePortWebKey, 0, http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		*reached = true
		rw.WriteHeader(http.StatusTeapot)
	}))
}

// zgcwkj on Discord: one reverse-proxy location for the page and the
// agents. `magpie web`'s port hands the gateway's client routes to the
// gateway this process serves, through the gateway's own guards.
func TestWebOnePortServedHere(t *testing.T) {
	onePortHome(t)
	g := gateway.New()
	prev := served.Swap(g)
	t.Cleanup(func() { served.Store(prev) })
	var reached bool
	onePortCases(t, onePortPage(&reached), &reached)
}

// With another magpie serving the gateway (the service, the app), the page
// passes the client routes on to it, and they are judged there as they
// came: Host, Origin and a proxy's forwarding headers kept, and one from
// another machine marked as such.
func TestWebOnePortServedElsewhere(t *testing.T) {
	onePortHome(t)
	other := httptest.NewServer(gateway.New().Front())
	t.Cleanup(other.Close)
	u, _ := url.Parse(other.URL)
	t.Setenv("MAGPIE_ADDR", u.Host)
	prev := served.Swap(nil)
	t.Cleanup(func() { served.Store(prev) })
	var reached bool
	onePortCases(t, onePortPage(&reached), &reached)
}

// No file of the page is at a client route, where the gateway would
// answer in its place.
func TestWebAssetsAreNoClientPath(t *testing.T) {
	fs.WalkDir(staticFS(), ".", func(p string, d fs.DirEntry, err error) error {
		if err == nil && gateway.ClientPath("/"+p) {
			t.Errorf("the page's /%s is a gateway client route", p)
		}
		return nil
	})
}
