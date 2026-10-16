package gui

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"errors"
	"fmt"
	"html"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/klauspost/compress/gzhttp"

	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/update"
)

// `magpie web` serves the window's page to a browser, for a computer that
// can't show the app — no desktop, WSL, a server reached over SSH — with
// the gateway beside it as the app has. What is the desktop's (the panel,
// the tray, the Dock) does nothing there; a folder is named, not opened.

// webHost is the Windows a browser tab stands for.
type webHost struct{ quit func() }

func (webHost) HidePanel()                       {}
func (webHost) ShowMain(string)                  {}
func (h webHost) Quit()                          { h.quit() }
func (webHost) OpenURL(string) error             { return nil } // the page opens links itself
func (webHost) Copy(string) bool                 { return false }
func (webHost) FitPanel(int, Glide)              {}
func (webHost) TintPanel([4]uint8, int) bool     { return false }
func (webHost) TintTitleBar([4]uint8, bool) bool { return false }
func (webHost) SetTextSize(int)                  {} // the browser zooms its own tab
func (webHost) OpenFolder(path string) error {
	return errors.New("in the browser magpie can't open folders: it is " + tilde(path))
}
func (webHost) ChooseFolder(string) (string, error) {
	return "", errors.New("in the browser magpie can't show a folder picker: type the folder's path")
}

// Web is a started `magpie web`: its address and the link that signs a
// browser in.
type Web struct {
	Addr, Link string
	key        string
	srv        *http.Server
	quit       chan struct{}
}

// webReexec is set when the page's restart to update has put the new
// version in: Wait then runs it in this one's place.
var webReexec atomic.Bool

// webRunKey hands a run's own key to the version it restarts into, so the
// browser's cookie still opens the page (#111).
const webRunKey = "MAGPIE_WEB_RUNKEY"

// StartWeb serves the page on addr (host:port), as the given version of
// magpie, which keeps itself up to date as the app does. Every request needs the
// key the link carries, taken once into a cookie other sites' pages can't
// send with a POST (every change is one), so neither a page elsewhere nor
// anyone else on the network reaches the settings and keys behind it.
func StartWeb(addr, version string) (*Web, error) {
	Version = version
	key, fixed, err := webKey()
	if err != nil {
		return nil, err
	}
	var keep time.Duration
	if fixed {
		keep = webCookieAge
	}
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return nil, err
	}
	// with --tls-cert, the page is HTTPS to other computers, as the
	// gateway is: it carries the settings and keys
	if ln, err = gateway.ServeTLS(ln); err != nil {
		return nil, err
	}
	w := &Web{Addr: ln.Addr().String(), key: key, quit: make(chan struct{})}
	host, port, _ := net.SplitHostPort(w.Addr)
	if ip := net.ParseIP(host); ip == nil || ip.IsUnspecified() {
		host = "127.0.0.1"
	}
	var once sync.Once
	quit := func() { once.Do(func() { close(w.quit) }) }
	// on loopback the page takes plain HTTP with TLS on too, which a
	// browser here opens without a certificate for 127.0.0.1
	scheme := "http"
	if ip := net.ParseIP(host); ip != nil && !ip.IsLoopback() {
		scheme = gateway.Scheme()
	}
	w.Link = scheme + "://" + net.JoinHostPort(host, port) + "/?k=" + url.QueryEscape(key)
	w.srv = &http.Server{
		Handler:           gateway.AllowGuard(webHandler(port, key, keep, Handler(webHost{quit}, startBackend()))),
		ReadHeaderTimeout: 30 * time.Second,
	}
	go w.srv.Serve(ln)
	// Settings' AutoRestart: magpie web runs on as the new version (#2096)
	updates.autoRestart = func() bool {
		if restartInto(true, false, "", true) {
			quit()
			return true
		}
		return false
	}
	updates.start()
	news.start()
	return w, nil
}

// ServesGateway says whether this process serves the gateway, which it
// doesn't while another magpie (the service, the app, magpie serve) has
// it: the page then shows that one's settings and accounts, read from the
// same files, and this one takes the gateway up once that one is gone.
func (w *Web) ServesGateway() bool { return served.Load() != nil }

// Wait returns once the page's Quit is used. After a restart to update it
// runs the new version in this one's place, with the same arguments and
// key, so the page open in the browser finds it where it was.
func (w *Web) Wait() error {
	<-w.quit
	time.Sleep(200 * time.Millisecond) // the answer to the Quit gets out
	w.srv.Close()
	if !webReexec.Load() {
		return nil
	}
	exe, err := update.Executable()
	if err != nil {
		return err
	}
	return update.Reexec(exe, os.Args[1:], append(os.Environ(), webRunKey+"="+w.key))
}

// webKeyMin is the shortest MAGPIE_WEB_KEY taken: it is all that stands
// between the network and the keys behind the page.
const webKeyMin = 16

// webCookieAge is how long a browser keeps a fixed key's cookie (the most
// Chrome allows); a run's own key lives in a cookie that goes with the
// browser session, as the key goes with the run.
const webCookieAge = 400 * 24 * time.Hour

// webKey is the key the link carries: a new one each run, unless
// MAGPIE_WEB_KEY names one — for a page kept running as a service, which
// browsers then stay signed in to across restarts. fixed says which.
func webKey() (key string, fixed bool, err error) {
	if k := os.Getenv(webRunKey); k != "" {
		os.Unsetenv(webRunKey) // the run's alone, not its children's
		if len(k) == 32 && strings.Trim(k, "0123456789abcdef") == "" && os.Getenv("MAGPIE_WEB_KEY") == "" {
			return k, false, nil
		}
	}
	if k := os.Getenv("MAGPIE_WEB_KEY"); k != "" {
		if err := CheckWebKey(k); err != nil {
			return "", false, err
		}
		return k, true, nil
	}
	return NewWebKey(), false, nil
}

// NewWebKey is a new random key for the page.
func NewWebKey() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// CheckWebKey says what is wrong with k as a MAGPIE_WEB_KEY, nil when
// nothing is.
func CheckWebKey(k string) error {
	if len(k) < webKeyMin {
		return fmt.Errorf("MAGPIE_WEB_KEY is %d characters; it needs at least %d", len(k), webKeyMin)
	}
	// a cookie can't hold a space, ; " \ or a comma: the browser would
	// be sent one that isn't the key, and turned away
	if strings.Trim(k, "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~") != "" {
		return errors.New("MAGPIE_WEB_KEY takes letters, digits and - . _ ~ only (openssl rand -hex 16 makes one)")
	}
	return nil
}

// webGuard lets through requests with the key: in the link's k, which it
// trades for a cookie (kept for keep, or the browser session when 0) and
// a clean address, or in that cookie. A page opened without it gets a
// form to paste the key into (webSignIn), which says why it was turned
// away, so a bookmark doesn't send anyone back to the start-up log.
func webGuard(cookie, key string, keep time.Duration, next http.Handler) http.Handler {
	same := func(v string) bool { return subtle.ConstantTimeCompare([]byte(v), []byte(key)) == 1 }
	signIn := func(rw http.ResponseWriter, r *http.Request, back string) {
		// Priority=High (Chrome's): the page's host is often an address
		// other services share on other ports, and a browser keeps a
		// limited number of cookies for one host, all ports together;
		// past it Chrome drops the low and medium ones first (#2033)
		rw.Header().Add("Set-Cookie", (&http.Cookie{Name: cookie, Value: key, Path: "/", HttpOnly: true, SameSite: http.SameSiteLaxMode, MaxAge: int(keep / time.Second)}).String()+"; Priority=High")
		http.Redirect(rw, r, back, http.StatusSeeOther)
	}
	return http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		// a plugin's page's files: a sandboxed frame's requests carry no
		// cookie, and its token, which only a signed-in /api hands out,
		// is the key (servePluginPage serves nothing without a good one)
		if strings.HasPrefix(r.URL.Path, pluginPagePrefix) {
			next.ServeHTTP(rw, r)
			return
		}
		if q := r.URL.Query(); q.Has("k") {
			if !same(q.Get("k")) {
				http.Error(rw, "this link's key is not this magpie web's: use the link it printed when it started", http.StatusUnauthorized)
				return
			}
			q.Del("k")
			u := *r.URL
			u.RawQuery = q.Encode()
			signIn(rw, r, u.RequestURI())
			return
		}
		if r.URL.Path == webSignInPath && r.Method == http.MethodPost {
			r.Body = http.MaxBytesReader(rw, r.Body, 4096)
			back := localPath(r.PostFormValue("back"))
			if !same(r.PostFormValue("key")) {
				webTurnedAway(rw, r, back, "That key is not this magpie web's.")
				return
			}
			signIn(rw, r, back)
			return
		}
		// A browser can hold more than one cookie of this name (one set
		// for another path or domain, an old run's): any that carries
		// the key will do, not only the first sent.
		sent := r.CookiesNamed(cookie)
		for _, c := range sent {
			if same(c.Value) {
				next.ServeHTTP(rw, r)
				return
			}
		}
		why, short := "This browser sent no sign-in cookie for this address. A browser keeps cookies per host, not per port, so another service on this host's other ports can take it away: by clearing its site data (a sign-out often does), or by filling the host's cookie limit. A page opened at another address than the one signed in at (an IP instead of a name) has none either.", "no cookie sent"
		if len(sent) > 0 {
			why, short = "This browser's sign-in cookie holds a key that isn't this magpie web's: the key changed after it signed in (MAGPIE_WEB_KEY changed, or a start without it made a new one).", "its cookie holds another key"
		}
		if r.Method != http.MethodGet || !strings.Contains(r.Header.Get("Accept"), "text/html") {
			http.Error(rw, "open the link magpie web printed when it started: it carries the key. "+why, http.StatusUnauthorized)
			return
		}
		log.Printf("magpie web: turned away %s from %s: %s", r.URL.Path, r.RemoteAddr, short)
		webTurnedAway(rw, r, r.URL.RequestURI(), why)
	})
}

// webSignInPath takes the key pasted into the form a page turned away shows.
const webSignInPath = "/_magpie/sign-in"

// localPath is back when it is a path on this server, else "/": the form's
// return address never leads to another site, nor to the form's own.
func localPath(back string) string {
	u, err := url.Parse(back)
	if err != nil || u.Scheme != "" || u.Host != "" || !strings.HasPrefix(back, "/") || strings.HasPrefix(back, "//") || strings.HasPrefix(back, "/\\") || u.Path == webSignInPath {
		return "/"
	}
	return back
}

// webTurnedAway is the 401 page a browser opening the page without the key
// sees: why, and a field for the key that signs it in and comes back to
// back. The key itself is never on it.
func webTurnedAway(rw http.ResponseWriter, r *http.Request, back, why string) {
	rw.Header().Set("Content-Type", "text/html; charset=utf-8")
	rw.Header().Set("Cache-Control", "no-store")
	rw.WriteHeader(http.StatusUnauthorized)
	fmt.Fprintf(rw, `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>magpie web</title>
<style>body{font:15px/1.5 system-ui,sans-serif;max-width:32em;margin:12vh auto;padding:0 16px;color:#222;background:#fafafa}@media(prefers-color-scheme:dark){body{color:#ddd;background:#1c1c1e}}input,button{font:inherit;padding:.4em .6em}input{width:100%%;box-sizing:border-box;margin:.5em 0}</style></head>
<body><h1>magpie web</h1><p>%s</p><p>Paste the key (MAGPIE_WEB_KEY, or the k= in the link magpie web printed when it started):</p>
<form method="post" action="%s"><input type="hidden" name="back" value="%s"><input type="password" name="key" autocomplete="current-password" autofocus required><button type="submit">Sign in</button></form></body></html>`,
		html.EscapeString(why), webSignInPath, html.EscapeString(back))
}

// NetworkLinks are the links to the page, served on port, that other
// machines open; key is the link's "/?k=…". MAGPIE_PUBLIC_URL with a port
// of its own names the host the container's ports are published on, so
// the page is at that host on its own port; one without (a reverse proxy's
// https://magpie.example.com, #372) is where the page is, path and all.
func NetworkLinks(port, key string) []string {
	if u, err := url.Parse(gateway.PublicURL()); err == nil && u.Host != "" && u.Port() == "" {
		return []string{strings.TrimRight(u.String(), "/") + key}
	}
	var out []string
	for _, a := range LANAddrs() {
		out = append(out, gateway.Scheme()+"://"+net.JoinHostPort(a, port)+key)
	}
	return out
}

// LANAddrs are this computer's addresses on the local network, for the
// links to print when the page is served there: MAGPIE_PUBLIC_URL's host
// when set (in a container, whose own addresses the network can't reach).
func LANAddrs() []string {
	if h := gateway.PublicHost(); h != "" {
		return []string{h}
	}
	var out []string
	as, _ := net.InterfaceAddrs()
	for _, a := range as {
		n, ok := a.(*net.IPNet)
		if !ok || n.IP.IsLoopback() || n.IP.To4() == nil || !n.IP.IsPrivate() {
			continue
		}
		out = append(out, n.IP.String())
	}
	return out
}

// webHandler is what `magpie web` on port serves: the page and its API
// behind the key, compressed for the browser.
func webHandler(port, key string, keep time.Duration, page http.Handler) http.Handler {
	return webCompressed(webGuard("magpie_web_"+port, key, keep, page))
}

// webCompressed gzips what `magpie web` sends a browser that takes it: the
// page's scripts, styles and SVG icons, and the API's JSON. They went out
// as they are, about 4 MB for a first load before any data, and the
// routing trace's JSON grows with every account a request could go to; a
// browser over a 3 Mbps link to a server waited for all of it (akic404 on
// Discord). They shrink to about a quarter, the JSON to a twentieth.
// Pictures already compressed go as they are.
//
// The app's own window gets the page from inside the process, with no
// network between, so only `magpie web` compresses.
//
// A gzipped file's ETag says so ("…-gzip"), as a validator of other bytes;
// asked for again with it, the file is still a 304. A few bytes of padding
// keyed to the content keep a response's compressed size from telling
// exactly what it holds (BREACH): the page carries keys.
func webCompressed(next http.Handler) http.Handler {
	wrap, err := gzhttp.NewWrapper(
		gzhttp.ContentTypeFilter(compressible),
		gzhttp.SuffixETag(gzipETag),
		gzhttp.RandomJitter(32, 0, false),
	)
	if err != nil {
		return next
	}
	gz := wrap(next)
	return http.HandlerFunc(func(rw http.ResponseWriter, r *http.Request) {
		if tag := r.Header.Get("If-None-Match"); strings.Contains(tag, gzipETag+`"`) {
			r.Header.Set("If-None-Match", strings.ReplaceAll(tag, gzipETag+`"`, `"`))
		}
		gz.ServeHTTP(rw, r)
	})
}

// gzipETag ends the ETag of a gzipped response.
const gzipETag = "-gzip"

// compressible: text, scripts, JSON and SVG; not PNG, WebP, ICO or JPEG.
func compressible(ct string) bool {
	ct = strings.ToLower(strings.TrimSpace(ct))
	if i := strings.IndexByte(ct, ';'); i >= 0 {
		ct = strings.TrimSpace(ct[:i])
	}
	return strings.HasPrefix(ct, "text/") || strings.HasSuffix(ct, "javascript") ||
		strings.HasSuffix(ct, "json") || ct == "image/svg+xml"
}

// isWeb: the page is served to a browser by `magpie web`.
func isWeb(w Windows) bool { _, ok := w.(webHost); return ok }
