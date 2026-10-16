package gui

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// A community plugin's own page in magpie's window (P4 of
// docs/subsystems/plugin-runtime.md): package.json's magpie.page, shown in
// a sandboxed frame (pluginpages.js). Its files are served under
// pluginPagePrefix and a token only this run of magpie hands out, with a
// policy that lets the page load nothing but its own files and reach
// nothing over the network; it has no way to magpie's /api (apiFramed),
// and what it may read of magpie's it asks the window for, which asks
// /api/plugin-pages/read for the reads its package declared.

// pluginPagePrefix is where a plugin's page's files are served:
// pluginPagePrefix + token + "/" + the file's path in the page's folder.
const pluginPagePrefix = "/_magpie/plugin/"

// pluginBridgePath is the window's side of the page's messages, served
// beside every page's files: a page loads it as
// <script src="_magpie/bridge.js">.
const pluginBridgePath = "_magpie/bridge.js"

//go:embed pagebridge.js
var pageBridgeJS []byte

// pageSecret keys this run's tokens: a token from an earlier run, or one
// made up, serves nothing.
var pageSecret = func() []byte {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		panic(err)
	}
	return b
}()

func pageToken(spec string) string {
	m := hmac.New(sha256.New, pageSecret)
	m.Write([]byte(spec))
	return hex.EncodeToString(m.Sum(nil)[:16])
}

// pluginPage is a plugin's page, of a plugin that is on.
type pluginPage struct {
	Spec  string   `json:"spec"`
	Name  string   `json:"name"`  // its package's
	Key   string   `json:"key"`   // the window's page key
	Title string   `json:"title"` // the page's, or the package's short name
	URL   string   `json:"url"`   // the page, under pluginPagePrefix
	Icon  string   `json:"icon,omitempty"`
	Reads []string `json:"reads"` // what it may read, of plugin.PageReads
	token string
	spec  *plugin.PageSpec
}

// pageKeyChars are what a window page's key can't have
var pageKeyChars = strings.NewReplacer("@", "", "/", "-", ".", "-", "_", "-", " ", "-")

// pluginPages are the pages of the plugins that are on, in plugins.json's
// order. A plugin switched off has none: its files aren't served.
func pluginPages() []pluginPage {
	var out []pluginPage
	seen := map[string]bool{}
	for _, e := range plugin.Load().Plugins {
		if e.Off {
			continue
		}
		spec, _ := plugin.WindowPage(plugin.Target(e.Spec))
		if spec == nil {
			continue
		}
		name := plugin.PackageName(e.Spec)
		key := "plugin-" + strings.Trim(pageKeyChars.Replace(strings.ToLower(plugin.ShortName(name))), "-")
		key = strings.Map(func(r rune) rune {
			if r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || r == '-' {
				return r
			}
			return -1
		}, key)
		for k, n := key, 2; seen[key]; n++ {
			key = fmt.Sprintf("%s-%d", k, n)
		}
		seen[key] = true
		tok := pageToken(e.Spec)
		rel, _ := filepath.Rel(spec.Dir, spec.File)
		p := pluginPage{Spec: e.Spec, Name: name, Key: key, Title: spec.Title, Reads: spec.Reads, token: tok, spec: spec,
			URL: pluginPagePrefix + tok + "/" + filepath.ToSlash(rel)}
		if p.Title == "" {
			p.Title = plugin.ShortName(name)
		}
		if p.Reads == nil {
			p.Reads = []string{}
		}
		if spec.Icon != "" {
			p.Icon = pluginPagePrefix + tok + "/" + spec.Icon
		}
		out = append(out, p)
	}
	return out
}

func pageOf(spec string) (pluginPage, bool) {
	for _, p := range pluginPages() {
		if p.Spec == spec {
			return p, true
		}
	}
	return pluginPage{}, false
}

// pagePolicy is what a plugin's page may load and reach: its own files,
// inline styles, data: pictures, and nothing over the network. sandbox
// keeps a page opened by itself (not in the window's frame) as shut in as
// the frame keeps it.
func pagePolicy(r *http.Request, tok string) string {
	own := "'self'"
	if r.Host != "" {
		// the page's own folder: not the window's files, nor /api
		own = r.Host + pluginPagePrefix + tok + "/"
		// WebKit does not match a scheme-less host source under wails://.
		// Only the native handler supplies this scheme; web keeps its
		// relative source so HTTP, HTTPS and reverse proxies still work.
		if scheme, _ := r.Context().Value(windowSchemeKey{}).(string); scheme != "" {
			own = scheme + "://" + own
		}
	}
	return strings.Join([]string{
		"sandbox allow-scripts",
		"default-src 'none'",
		"script-src " + own,
		"style-src " + own + " 'unsafe-inline'",
		"img-src " + own + " data:",
		"font-src " + own,
		"media-src " + own,
		"connect-src 'none'",
		"frame-src 'none'",
		"worker-src 'none'",
		"object-src 'none'",
		"form-action 'none'",
		"base-uri 'none'",
		"frame-ancestors 'self'",
	}, "; ")
}

// servePluginPage serves a file of a plugin's page, by its token.
func servePluginPage(rw http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		http.Error(rw, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	rest := strings.TrimPrefix(r.URL.Path, pluginPagePrefix)
	tok, rel, ok := strings.Cut(rest, "/")
	if !ok || len(tok) != 32 {
		http.NotFound(rw, r)
		return
	}
	var page *pluginPage
	for _, p := range pluginPages() {
		if hmac.Equal([]byte(p.token), []byte(tok)) {
			page = &p
			break
		}
	}
	if page == nil {
		http.NotFound(rw, r)
		return
	}
	h := rw.Header()
	h.Set("Content-Security-Policy", pagePolicy(r, tok))
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("Referrer-Policy", "no-referrer")
	// no Cross-Origin-Resource-Policy: to the page's sandboxed frame, an
	// origin of its own, its own files are another origin's, and
	// same-origin (or same-site) would block every one of them
	h.Set("Cache-Control", "no-cache")
	if rel == pluginBridgePath {
		h.Set("Content-Type", "text/javascript; charset=utf-8")
		http.ServeContent(rw, r, "bridge.js", time.Time{}, strings.NewReader(string(pageBridgeJS)))
		return
	}
	file, ok := page.spec.PageFile(rel)
	if !ok {
		http.NotFound(rw, r)
		return
	}
	// ServeContent, not ServeFile: ServeFile sends .../index.html to ./,
	// which a page's own links needn't follow.
	f, err := os.Open(file)
	if err != nil {
		http.NotFound(rw, r)
		return
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil || st.IsDir() {
		http.NotFound(rw, r)
		return
	}
	http.ServeContent(rw, r, filepath.Base(file), st.ModTime(), f)
}

// apiFramed is whether r is a request for /api that no page of the window's
// own makes: one from a sandboxed frame (Origin: null, which a plugin's
// page's fetch would send were its policy not to stop it), one that loads
// /api into a frame, or one another site's page sends. The desktop
// window's /api has no key, so this is what keeps a plugin's page out.
func apiFramed(r *http.Request) bool {
	if !strings.HasPrefix(r.URL.Path, "/api/") {
		return false
	}
	if r.Header.Get("Origin") == "null" {
		return true
	}
	switch r.Header.Get("Sec-Fetch-Dest") {
	case "iframe", "frame", "embed", "object":
		return true
	}
	return r.Header.Get("Sec-Fetch-Site") == "cross-site" && r.Header.Get("Sec-Fetch-Dest") != "document"
}

func pluginPageRoutes(mux *http.ServeMux) {
	mux.HandleFunc(pluginPagePrefix, servePluginPage)
	mux.HandleFunc("GET /api/plugin-pages", func(rw http.ResponseWriter, r *http.Request) {
		out := pluginPages()
		if out == nil {
			out = []pluginPage{}
		}
		writeJSON(rw, out)
	})
	// what a page asked the window to read: only what its package
	// declared, and never a secret
	mux.HandleFunc("POST /api/plugin-pages/read", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Spec   string         `json:"spec"`
			Read   string         `json:"read"`
			Params map[string]any `json:"params"`
		}
		if err := json.NewDecoder(http.MaxBytesReader(rw, r.Body, 64<<10)).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		p, ok := pageOf(in.Spec)
		if !ok {
			fail(rw, errors.New("no plugin page "+in.Spec))
			return
		}
		if !slices.Contains(p.Reads, in.Read) {
			rw.Header().Set("Content-Type", "application/json")
			rw.WriteHeader(http.StatusForbidden)
			_ = json.NewEncoder(rw).Encode(map[string]string{"error": fmt.Sprintf("%s's package doesn't ask to read %q", p.Name, in.Read)})
			return
		}
		v, err := pageRead(in.Read, in.Params)
		if err != nil {
			fail(rw, err)
			return
		}
		writeJSON(rw, v)
	})
	// a page's own options, as plugins.json keeps them: no other plugin's
	mux.HandleFunc("POST /api/plugin-pages/options", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			Spec    string          `json:"spec"`
			Options json.RawMessage `json:"options"`
		}
		if err := json.NewDecoder(http.MaxBytesReader(rw, r.Body, 64<<10)).Decode(&in); err != nil {
			fail(rw, err)
			return
		}
		if _, ok := pageOf(in.Spec); !ok {
			fail(rw, errors.New("no plugin page "+in.Spec))
			return
		}
		if len(in.Options) > 0 && string(in.Options) != "null" {
			var opts map[string]any
			if err := json.Unmarshal(in.Options, &opts); err != nil {
				fail(rw, errors.New("a page's options are an object"))
				return
			}
			// a page that sets them over and over would start the
			// host, every plugin's, again each time
			if !pageSetAllowed(in.Spec, time.Now()) {
				http.Error(rw, "a page sets its options once a second at most", http.StatusTooManyRequests)
				return
			}
			if err := plugin.SetPageOptions(in.Spec, opts); err != nil {
				fail(rw, err)
				return
			}
		}
		for _, e := range plugin.Load().Plugins {
			if e.Spec == in.Spec {
				opts := e.Options
				if opts == nil {
					opts = map[string]any{}
				}
				writeJSON(rw, opts)
				return
			}
		}
		writeJSON(rw, map[string]any{})
	})
}

var pageSets = struct {
	sync.Mutex
	at map[string]time.Time
}{at: map[string]time.Time{}}

// pageSetAllowed is whether spec's page may set its options now: a second
// after it last did.
func pageSetAllowed(spec string, now time.Time) bool {
	pageSets.Lock()
	defer pageSets.Unlock()
	if last, ok := pageSets.at[spec]; ok && now.Sub(last) < time.Second {
		return false
	}
	pageSets.at[spec] = now
	return true
}

// pageRead is one of plugin.PageReads.
func pageRead(read string, params map[string]any) (any, error) {
	switch read {
	case "providers:list":
		// a name and an id each: no key, no account, no address
		type prov struct {
			ID   string `json:"id"`
			Name string `json:"name"`
		}
		out := []prov{}
		for _, p := range provider.All() {
			out = append(out, prov{ID: p.ID, Name: p.Name})
		}
		return out, nil
	case "usage:summary":
		period := usage.Today
		if s, _ := params["period"].(string); s != "" {
			period = usage.Period(s)
		}
		if !slices.Contains([]usage.Period{usage.Today, usage.Week, usage.Month}, period) {
			return nil, fmt.Errorf("no period %q: today, 7d or 30d", period)
		}
		s := usage.Summarize(period)
		type model struct {
			Model  string  `json:"model"`
			Calls  int     `json:"calls"`
			Input  int     `json:"input"`
			Output int     `json:"output"`
			Cost   float64 `json:"cost"`
		}
		// the totals and each model's, never a key's, an account's or a
		// session's
		out := struct {
			Period string    `json:"period"`
			Since  time.Time `json:"since"`
			Calls  int       `json:"calls"`
			Errors int       `json:"errors"`
			Input  int       `json:"input"`
			Output int       `json:"output"`
			Cache  int       `json:"cacheRead"`
			Cost   float64   `json:"cost"`
			Models []model   `json:"models"`
		}{Period: string(period), Since: s.Since, Calls: s.Calls, Errors: s.Errors, Input: s.Input, Output: s.Output, Cache: s.CacheRead, Cost: s.Cost, Models: []model{}}
		for i, g := range s.Models {
			if i == 20 {
				break
			}
			m := g.Model
			if m == "" {
				m = g.ID
			}
			out.Models = append(out.Models, model{Model: m, Calls: g.Calls, Input: g.Input, Output: g.Output, Cost: g.Cost})
		}
		return out, nil
	}
	return nil, fmt.Errorf("no read %q", read)
}
