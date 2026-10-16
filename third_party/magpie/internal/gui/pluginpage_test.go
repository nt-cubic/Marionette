package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// pagePlugins writes plugins.json with the example page plugin
// (testdata/page-plugin), on, and a copy of it switched off; it answers
// their specs.
func pagePlugins(t *testing.T) (on, off string) {
	t.Helper()
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(h, ".local", "share"))
	t.Setenv("MAGPIE_BUN", "")
	t.Setenv("PATH", "")
	copyDir := func(to string) {
		t.Helper()
		err := filepath.WalkDir("testdata/page-plugin", func(p string, d os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			rel, _ := filepath.Rel("testdata/page-plugin", p)
			if d.IsDir() {
				return os.MkdirAll(filepath.Join(to, rel), 0o755)
			}
			b, err := os.ReadFile(p)
			if err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(to, rel), b, 0o644)
		})
		if err != nil {
			t.Fatal(err)
		}
	}
	on, off = filepath.Join(h, "src", "glance"), filepath.Join(h, "src", "glance-off")
	copyDir(on)
	copyDir(off)
	// a file of the plugin's own, but out of its page's folder, and one out
	// of the plugin, which a link in the page's folder points to
	if err := os.WriteFile(filepath.Join(h, "secret.txt"), []byte("SECRET"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(h, "secret.txt"), filepath.Join(on, "page", "link.txt")); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(settings.Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	b, _ := json.Marshal(map[string]any{"plugins": []map[string]any{{"spec": on}, {"spec": off, "off": true}}})
	if err := os.WriteFile(filepath.Join(settings.Dir(), "plugins.json"), b, 0o644); err != nil {
		t.Fatal(err)
	}
	return on, off
}

func pageMux() http.Handler {
	mux := http.NewServeMux()
	pluginPageRoutes(mux)
	mux.HandleFunc("GET /api/state", func(rw http.ResponseWriter, r *http.Request) {
		writeJSON(rw, map[string]string{"secret": "the window's"})
	})
	return noFramedAPI(mux)
}

func do(h http.Handler, r *http.Request) *httptest.ResponseRecorder {
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	return w
}

// A plugin's page's files are served by a token of this run, with a policy
// that keeps it to its own folder and off the network; nothing outside its
// page's folder is served, nor anything of a plugin switched off, nor
// anything by a token made up.
func TestPluginPageServedShutIn(t *testing.T) {
	on, off := pagePlugins(t)
	h := pageMux()
	var pages []pluginPage
	w := do(h, httptest.NewRequest("GET", "/api/plugin-pages", nil))
	if err := json.Unmarshal(w.Body.Bytes(), &pages); err != nil || len(pages) != 1 {
		t.Fatalf("pages: %s %v", w.Body, err)
	}
	p := pages[0]
	if p.Spec != on || p.Key != "plugin-magpie-usage-glance" || p.Title != "Usage glance" || len(p.Reads) != 1 || p.Reads[0] != "usage:summary" {
		t.Fatalf("page %+v", p)
	}
	tok := strings.Split(strings.TrimPrefix(p.URL, pluginPagePrefix), "/")[0]
	r := httptest.NewRequest("GET", p.URL, nil)
	r.Host = "127.0.0.1:3437"
	w = do(h, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), "Usage glance") {
		t.Fatalf("the page: %d %s", w.Code, w.Body)
	}
	csp := w.Header().Get("Content-Security-Policy")
	for _, want := range []string{"sandbox allow-scripts", "default-src 'none'", "connect-src 'none'", "script-src 127.0.0.1:3437" + pluginPagePrefix + tok + "/;", "form-action 'none'", "frame-src 'none'"} {
		if !strings.Contains(csp, want) {
			t.Errorf("policy %q has no %q", csp, want)
		}
	}
	if strings.Contains(csp, "allow-same-origin") || strings.Contains(csp, "'unsafe-eval'") {
		t.Errorf("policy %q lets the page out", csp)
	}
	// the sandboxed frame is an origin of its own: a resource policy of
	// same-origin or same-site blocks the page's own scripts and styles
	if c := w.Header().Get("Cross-Origin-Resource-Policy"); c != "" && c != "cross-origin" {
		t.Errorf("Cross-Origin-Resource-Policy %q keeps the page from its own files", c)
	}
	if w.Header().Get("X-Content-Type-Options") != "nosniff" {
		t.Error("no nosniff")
	}
	if w := do(h, httptest.NewRequest("GET", pluginPagePrefix+tok+"/"+pluginBridgePath, nil)); w.Code != 200 || !strings.Contains(w.Body.String(), "postMessage") {
		t.Errorf("the bridge: %d", w.Code)
	}
	// out of the page's folder, through a link, by another token
	for _, path := range []string{
		pluginPagePrefix + tok + "/../package.json",
		pluginPagePrefix + tok + "/link.txt",
		pluginPagePrefix + pageToken(off) + "/index.html",
		pluginPagePrefix + strings.Repeat("0", 32) + "/index.html",
		pluginPagePrefix + tok,
	} {
		r := httptest.NewRequest("GET", "/", nil)
		r.URL.Path = path
		w := httptest.NewRecorder()
		servePluginPage(w, r)
		if w.Code == 200 || strings.Contains(w.Body.String(), "SECRET") || strings.Contains(w.Body.String(), `"name"`) {
			t.Errorf("%s: %d %s", path, w.Code, w.Body)
		}
	}
}

// What a page didn't declare it reads is refused by the server too, not
// only by the window; a page sets its own options and no other plugin's.
func TestPluginPageReadsWhatItDeclared(t *testing.T) {
	on, off := pagePlugins(t)
	h := pageMux()
	post := func(path, body string) *httptest.ResponseRecorder {
		r := httptest.NewRequest("POST", path, strings.NewReader(body))
		r.Header.Set("Content-Type", "application/json")
		return do(h, r)
	}
	spec, _ := json.Marshal(on)
	if w := post("/api/plugin-pages/read", `{"spec":`+string(spec)+`,"read":"providers:list"}`); w.Code != http.StatusForbidden {
		t.Errorf("an undeclared read: %d %s", w.Code, w.Body)
	}
	w := post("/api/plugin-pages/read", `{"spec":`+string(spec)+`,"read":"usage:summary","params":{"period":"today"}}`)
	var sum map[string]any
	if w.Code != 200 || json.Unmarshal(w.Body.Bytes(), &sum) != nil || sum["period"] != "today" {
		t.Errorf("a declared read: %d %s", w.Code, w.Body)
	}
	for _, k := range []string{"callerKeys", "accounts", "sessions", "providerKeys"} {
		if _, ok := sum[k]; ok {
			t.Errorf("usage:summary gave the page %s", k)
		}
	}
	offSpec, _ := json.Marshal(off)
	if w := post("/api/plugin-pages/read", `{"spec":`+string(offSpec)+`,"read":"usage:summary"}`); w.Code == 200 {
		t.Errorf("a plugin switched off read: %d", w.Code)
	}
	if w := post("/api/plugin-pages/options", `{"spec":`+string(spec)+`,"options":{"hideCost":true}}`); w.Code != 200 || !strings.Contains(w.Body.String(), `"hideCost":true`) {
		t.Fatalf("options set: %d %s", w.Code, w.Body)
	}
	if w := post("/api/plugin-pages/options", `{"spec":`+string(spec)+`,"options":{"hideCost":false}}`); w.Code != http.StatusTooManyRequests {
		t.Errorf("a second set within the second: %d", w.Code)
	}
	if w := post("/api/plugin-pages/options", `{"spec":`+string(spec)+`}`); !strings.Contains(w.Body.String(), `"hideCost":true`) {
		t.Errorf("options get: %s", w.Body)
	}
	if w := post("/api/plugin-pages/options", `{"spec":`+string(offSpec)+`,"options":{"x":1}}`); w.Code == 200 {
		t.Errorf("another plugin's options set: %d", w.Code)
	}
	if !pageSetAllowed("x", time.Now()) || pageSetAllowed("x", time.Now()) || !pageSetAllowed("x", time.Now().Add(2*time.Second)) {
		t.Error("pageSetAllowed")
	}
}

// The desktop window's /api has no key: what keeps a plugin's page off it
// is that every way a sandboxed frame can ask is turned away, a fetch
// (Origin: null), a frame of /api, another site's page; the window's own
// requests, and a link opened from elsewhere, go on.
func TestPluginPageNoAPI(t *testing.T) {
	pagePlugins(t)
	h := pageMux()
	ask := func(set map[string]string) int {
		r := httptest.NewRequest("GET", "/api/state", nil)
		for k, v := range set {
			r.Header.Set(k, v)
		}
		return do(h, r).Code
	}
	for _, c := range []map[string]string{
		{"Origin": "null"},
		{"Origin": "null", "Sec-Fetch-Site": "cross-site", "Sec-Fetch-Mode": "cors", "Sec-Fetch-Dest": "empty"},
		{"Sec-Fetch-Dest": "iframe", "Sec-Fetch-Site": "same-origin"},
		{"Sec-Fetch-Dest": "image", "Sec-Fetch-Site": "cross-site"},
		{"Sec-Fetch-Dest": "script", "Sec-Fetch-Site": "cross-site"},
	} {
		if code := ask(c); code != http.StatusForbidden {
			t.Errorf("%v: %d", c, code)
		}
	}
	// the window's own handler, not only this test's
	r := httptest.NewRequest("GET", "/api/plugin-pages", nil)
	r.Header.Set("Origin", "null")
	if w := do(Handler(nil, nil), r); w.Code != http.StatusForbidden {
		t.Errorf("the window's /api to a sandboxed frame: %d", w.Code)
	}
	for _, c := range []map[string]string{
		{},
		{"Sec-Fetch-Site": "same-origin", "Sec-Fetch-Dest": "empty", "Origin": "http://127.0.0.1:3437"},
		{"Sec-Fetch-Site": "cross-site", "Sec-Fetch-Dest": "document"},
	} {
		if code := ask(c); code != 200 {
			t.Errorf("%v: %d", c, code)
		}
	}
}

// magpie web's guard lets a page's files through without the cookie a
// sandboxed frame never sends, and only those: a made-up token is served
// nothing, and the rest of the page still wants the key.
func TestPluginPageWebGuard(t *testing.T) {
	pagePlugins(t)
	g := webGuard("magpie_web_1", "key", 0, pageMux())
	var pages []pluginPage
	_ = json.Unmarshal(do(pageMux(), httptest.NewRequest("GET", "/api/plugin-pages", nil)).Body.Bytes(), &pages)
	if len(pages) != 1 {
		t.Fatal("no page")
	}
	if w := do(g, httptest.NewRequest("GET", pages[0].URL, nil)); w.Code != 200 {
		t.Errorf("the page's file without the cookie: %d", w.Code)
	}
	if w := do(g, httptest.NewRequest("GET", pluginPagePrefix+strings.Repeat("a", 32)+"/index.html", nil)); w.Code != 404 {
		t.Errorf("a made-up token: %d", w.Code)
	}
	if w := do(g, httptest.NewRequest("GET", "/api/plugin-pages", nil)); w.Code != http.StatusUnauthorized {
		t.Errorf("/api without the cookie: %d", w.Code)
	}
}
