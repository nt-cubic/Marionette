package gateway

import (
	"context"
	"crypto/subtle"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/budget"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// The gateway accepts named caller keys locally and, while shared or on the
// network, remotely. Loopback clients may still use any token, including a
// stale named key.

// listenAddr is where the gateway listens: every interface while it is
// shared, on its port, else its address. A host MAGPIE_ADDR names itself
// (127.0.0.1 behind Tailscale Serve, one interface's address) is kept
// while shared too: sharing asks remote callers for a gateway key, and
// never widens the listener past the address the user chose (#1112).
func listenAddr() string {
	if s := settings.Load(); s.LAN && pinnedHost() == "" {
		return "0.0.0.0:" + Port()
	}
	return Addr()
}

// pinnedHost is the host MAGPIE_ADDR names, "" when it isn't set or names
// every interface (0.0.0.0, ::, or no host).
func pinnedHost() string {
	a := os.Getenv("MAGPIE_ADDR")
	if a == "" {
		return ""
	}
	h, _, err := net.SplitHostPort(a)
	if err != nil || h == "" {
		return ""
	}
	if ip := net.ParseIP(h); ip != nil && ip.IsUnspecified() {
		return ""
	}
	return h
}

// Port is the gateway's port.
func Port() string {
	_, p, err := net.SplitHostPort(Addr())
	if err != nil {
		return strconv.Itoa(settings.DefaultPort)
	}
	return p
}

func migrateLANKey() error { return access.MigrateLegacyLANKey() }

func migrateLANKeyBestEffort() {
	access.MigrateLegacyLANKeyBestEffort()
}

func logInvalidPublicURL() {
	log.Printf("ignoring invalid MAGPIE_PUBLIC_URL: expected an HTTP or HTTPS address with a host")
}

var warnInvalidPublicURL = sync.OnceFunc(logInvalidPublicURL)

// publicURL is the validated MAGPIE_PUBLIC_URL, with a scheme and without
// trailing slashes. An invalid value is treated as unset and warned once.
func publicURL() string {
	u := strings.TrimSpace(os.Getenv("MAGPIE_PUBLIC_URL"))
	if u == "" {
		return ""
	}
	// Keep a scheme-only value invalid after trimming its slashes.
	hasScheme := strings.Contains(u, "://")
	u = strings.TrimRight(u, "/")
	if !hasScheme {
		u = Scheme() + "://" + u
	}
	parsed, err := url.Parse(u)
	// A query, a fragment or credentials would be lost or shown when the
	// address is joined with a path, so only a plain base URL is taken.
	if err != nil || parsed.Host == "" || (parsed.Scheme != "http" && parsed.Scheme != "https") ||
		parsed.RawQuery != "" || parsed.Fragment != "" || parsed.User != nil {
		warnInvalidPublicURL()
		return ""
	}
	return u
}

// PublicURL is MAGPIE_PUBLIC_URL, with a scheme; "" when it isn't set.
func PublicURL() string { return publicURL() }

// OnNetwork: MAGPIE_ADDR puts the gateway beyond loopback (0.0.0.0 in the
// Docker image, a server's or one interface's address), so it is reached
// from other machines, and from a container's host, whether or not it is
// shared from Settings. Such callers need an enabled gateway key, as they
// do while it is shared: nothing that reaches it from elsewhere is let in
// on any key.
func OnNetwork() bool {
	h, _, err := net.SplitHostPort(Addr())
	return err == nil && h != "localhost" && !net.ParseIP(h).IsLoopback()
}

// remoteKeyed: requests from another machine are answered, with an enabled
// gateway key — while the gateway is shared, or listens on the network.
func remoteKeyed() bool { return settings.Load().LAN || OnNetwork() }

// PublicHost is MAGPIE_PUBLIC_URL's host, "" when it isn't set.
func PublicHost() string {
	u, err := url.Parse(publicURL())
	if err != nil {
		return ""
	}
	return u.Hostname()
}

// ContainerAddrs: the addresses magpie finds for itself are a container's,
// which other machines can't reach, and MAGPIE_PUBLIC_URL doesn't say the
// host's.
func ContainerAddrs() bool { return publicURL() == "" && inContainer("/") }

// InContainer: this magpie runs in a container (Docker, Podman, a
// Kubernetes pod), whose image is what gets updated, not its binary.
func InContainer() bool { return inContainer("/") }

// inContainer: the system under root is a container's — Docker's or
// Podman's marker file, or a container runtime in PID 1's cgroup.
func inContainer(root string) bool {
	for _, f := range []string{".dockerenv", "run/.containerenv"} {
		if _, err := os.Stat(filepath.Join(root, f)); err == nil {
			return true
		}
	}
	b, _ := os.ReadFile(filepath.Join(root, "proc/1/cgroup"))
	for _, w := range []string{"docker", "containerd", "kubepods", "libpod", "lxc"} {
		if strings.Contains(string(b), w) {
			return true
		}
	}
	return false
}

// LANURLs are the addresses other machines on the network reach the
// gateway at: MAGPIE_PUBLIC_URL when set, else one per IPv4 address this
// computer has there.
func LANURLs() []string {
	if u := publicURL(); u != "" {
		return []string{u}
	}
	// a gateway MAGPIE_ADDR keeps on one host is reached there alone; on
	// loopback, only through a proxy, whose address MAGPIE_PUBLIC_URL says
	if h := pinnedHost(); h != "" {
		if ip := net.ParseIP(h); h == "localhost" || ip != nil && ip.IsLoopback() {
			return nil
		}
		return []string{Scheme() + "://" + net.JoinHostPort(h, Port())}
	}
	var out []string
	ifs, _ := net.Interfaces()
	for _, i := range ifs {
		if i.Flags&net.FlagUp == 0 || i.Flags&net.FlagLoopback != 0 {
			continue
		}
		addrs, _ := i.Addrs()
		for _, a := range addrs {
			n, ok := a.(*net.IPNet)
			if !ok || n.IP.To4() == nil || n.IP.IsLinkLocalUnicast() {
				continue
			}
			out = append(out, Scheme()+"://"+net.JoinHostPort(n.IP.String(), Port()))
		}
	}
	return out
}

// listen listens on addr, serving TLS to other computers when the gateway
// does; under lnMu, or before serving.
func (s *Server) listen(addr string) (net.Listener, error) {
	ln, err := Listen(addr)
	if err != nil || s.tls == nil {
		return ln, err
	}
	return newTLSListener(ln, s.tls), nil
}

// Relisten moves the gateway to where settings now say it listens — onto
// the network or back to loopback, or to another port. Requests in flight
// finish.
func (s *Server) Relisten() error {
	migrateLANKeyBestEffort()
	s.lnMu.Lock()
	defer s.lnMu.Unlock()
	if s.ln == nil {
		return nil // not serving
	}
	to := listenAddr()
	if s.ln.Addr().String() == to {
		return nil
	}
	was := s.ln.Addr().String()
	// another port (Settings' port changed): the new one is taken up before
	// the old one lets go, so one the gateway can't have leaves it serving
	// where it was
	if _, p, _ := net.SplitHostPort(was); p != Port() {
		ln, err := s.listen(to)
		if err != nil {
			return err
		}
		old := s.ln
		s.ln = ln
		old.Close()
		return nil
	}
	// the port is the same, so the old one goes first
	s.ln.Close()
	ln, err := s.listen(to)
	if err != nil {
		if ln, _ = s.listen(was); ln == nil {
			return err
		}
		s.ln = ln
		return err
	}
	s.ln = ln
	return nil
}

// lanGuard: a request from an address the allow list (allow.go) leaves out
// is refused first, whatever key it carries. A request from another machine — any peer that isn't
// loopback, and a proxy's or tunnel's on loopback — needs an enabled gateway
// key, and is answered at all only while the gateway is shared or listens on
// the network (OnNetwork). Loopback clients may still use any token,
// including a stale named key.
func lanGuard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if ownCallback(r) {
			next.ServeHTTP(w, r)
			return
		}
		// the allow list comes before the key: one that leaked is of no
		// use from an address left out of it
		if refuseNotAllowed(w, r) {
			return
		}
		remote := !local(r)
		if !remote && rebound(r) {
			log.Printf("refused %s %s: a web page reached loopback under the name %q", r.Method, r.URL.Path, r.Host)
			writeError(w, provider.Chat, http.StatusForbidden, "magpie doesn't answer a browser that reached it as "+r.Host+" (a web site's name pointed at this computer): call it as localhost or 127.0.0.1, or send an enabled gateway key")
			return
		}
		if remote && !remoteKeyed() {
			if proxied(r) {
				log.Printf("refused %s %s through a proxy or tunnel: magpie isn't shared", r.Method, r.URL.Path)
				http.Error(w, "this request came through a proxy or tunnel (it carries a forwarding header such as X-Forwarded-For or Cf-Connecting-IP), so magpie answers it as one from another machine: only while Settings → Network and sharing → Share on local network is on, with an enabled gateway key (Gateway → Gateway keys) sent as Authorization: Bearer <key> or x-api-key", http.StatusForbidden)
				return
			}
			http.Error(w, "magpie isn't shared on the local network", http.StatusForbidden)
			return
		}
		if remote || managedKey(r) {
			var ok bool
			r, ok = identifyCaller(w, r)
			if !ok {
				return
			}
			if remote {
				r = r.WithContext(context.WithValue(r.Context(), lanKeyed{}, true))
			}
		}
		next.ServeHTTP(w, r)
	})
}

// callbacks are the callback tokens of the Claude subscription runs now
// live (addCallback, dropCallback).
var callbacks = struct {
	sync.Mutex
	tokens map[string]bool
}{tokens: map[string]bool{}}

// addCallback makes token, a run's callback token (randomToken: 24 bytes
// from crypto/rand), one ownCallback takes, until dropCallback.
func addCallback(token string) {
	callbacks.Lock()
	callbacks.tokens[token] = true
	callbacks.Unlock()
}

func dropCallback(token string) {
	callbacks.Lock()
	delete(callbacks.tokens, token)
	callbacks.Unlock()
}

// ownCallback: r is the MCP helper of a Claude Code magpie runs handing a
// tool call back, on the path it was given — POST claudeMCPPath and the
// callback token of a run now live, nothing else. It is let in without a
// gateway key from wherever it comes: the helper calls the address the
// gateway listens on, and with MAGPIE_ADDR on one interface's address (a
// LAN, Tailscale or loopback-alias IP), which isn't loopback, it was asked
// for a key it doesn't have (401). Its tool call never reached the run,
// so every turn with tool results found no run waiting and started a new
// Claude Code told the whole conversation, its prompt cache written anew
// (#1546, jiayeZhu). The token, unguessable and handed to the helper in a
// file only this user reads, is what lets it in: a wrong, spent or missing
// one, another method, or any other path is judged as every request is.
func ownCallback(r *http.Request) bool {
	if r.Method != http.MethodPost || r.URL.RawPath != "" {
		return false
	}
	token, ok := strings.CutPrefix(r.URL.Path, claudeMCPPath)
	if !ok || token == "" {
		return false
	}
	callbacks.Lock()
	defer callbacks.Unlock()
	found := false
	for t := range callbacks.tokens {
		if subtle.ConstantTimeCompare([]byte(t), []byte(token)) == 1 {
			found = true
		}
	}
	return found
}

// rebound: a browser sent this request to loopback under a hostname that
// isn't this computer's — a web site whose DNS answered 127.0.0.1 after its
// page loaded (DNS rebinding). The page is then same-origin with the
// gateway: it sends no Origin on a GET, or one equal to the Host, so
// corsGuard lets it by, and on loopback a call needs no key, so the page
// could read the models and quotas and spend the user's subscriptions.
// Agents call magpie as localhost or an IP, and send none of a browser's
// marks; a hostname of this computer's own (MAGPIE_PUBLIC_URL's,
// MAGPIE_ADDR's, its own name, a container runtime's name for the host)
// isn't a web site's. An enabled gateway key, which such a page can't
// know, still lets a call in.
func rebound(r *http.Request) bool {
	if !fromBrowser(r) || ownHost(r.Host) {
		return false
	}
	return !slices.ContainsFunc(callerKeys(r), func(k string) bool { _, ok := access.Authenticate(k); return ok })
}

// fromBrowser: the request carries what a browser puts on every request
// and a page's script can't take off.
func fromBrowser(r *http.Request) bool {
	return r.Header.Get("Origin") != "" || r.Header.Get("Sec-Fetch-Site") != "" ||
		r.Header.Get("Sec-Fetch-Mode") != "" || strings.HasPrefix(r.UserAgent(), "Mozilla/")
}

// containerHostNames are the names container runtimes give the host.
var containerHostNames = []string{"host.docker.internal", "gateway.docker.internal", "host.containers.internal", "host.orb.internal", "docker.for.mac.localhost"}

// ownHost: host (a Host header) names this computer — an IP, localhost,
// or one of the names above. A subdomain of localhost doesn't (#2256):
// Safari and other WebKit views ask the DNS for it, so a page served at
// one from anywhere can have it turned to 127.0.0.1 after it loads.
func ownHost(host string) bool {
	if h, _, err := net.SplitHostPort(host); err == nil {
		host = h
	}
	host = strings.TrimSuffix(strings.ToLower(strings.Trim(host, "[]")), ".")
	if host == "" || net.ParseIP(host) != nil || host == "localhost" || slices.Contains(containerHostNames, host) {
		return true
	}
	if strings.EqualFold(host, PublicHost()) {
		return true
	}
	if h, _, err := net.SplitHostPort(Addr()); err == nil && strings.EqualFold(host, h) {
		return true
	}
	if name, err := os.Hostname(); err == nil {
		name = strings.TrimSuffix(strings.ToLower(name), ".local")
		if name != "" && (host == name || host == name+".local") {
			return true
		}
	}
	return false
}

// callerGuard also covers embedded handlers used by the web app and tests.
func callerGuard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if access.Caller(r.Context()).KeyID == "" && managedKey(r) && (local(r) || remoteKeyed()) {
			var ok bool
			r, ok = identifyCaller(w, r)
			if !ok {
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func identifyCaller(w http.ResponseWriter, r *http.Request) (*http.Request, bool) {
	var who access.Identity
	ok := false
	for _, k := range callerKeys(r) {
		if who, ok = access.Authenticate(k); ok {
			break
		}
	}
	if !ok && !local(r) {
		if over, retry := tooManyAuthFails(); over {
			// A flood of wrong keys: stop answering each one (no 401 oracle,
			// no unbounded work) until the bucket refills. A valid key never
			// reaches here, so a real caller is not held.
			w.Header().Set("Retry-After", strconv.Itoa(int(retry.Seconds())+1))
			writeError(w, provider.Chat, http.StatusTooManyRequests, "too many failed authentications; slow down and send an enabled magpie gateway key")
			return r, false
		}
		msg := refusedKey(callerKey(r))
		log.Printf("refused %s %s from %s: %s", r.Method, r.URL.Path, r.RemoteAddr, msg)
		writeError(w, provider.Chat, http.StatusUnauthorized, msg)
		return r, false
	}
	if ok {
		r = r.WithContext(access.WithIdentity(r.Context(), who))
	}
	r.Header = r.Header.Clone()
	r.Header.Set("Authorization", "Bearer "+Token)
	for _, h := range []string{"x-api-key", "x-goog-api-key"} {
		if r.Header.Get(h) != "" {
			r.Header.Set(h, Token)
		}
	}
	if q := r.URL.Query(); q.Get("key") != "" {
		q.Set("key", Token)
		u := *r.URL
		u.RawQuery = q.Encode()
		r.URL = &u
	}
	return r, true
}

// accountOf is the subscription account p answers as, named as the Routing
// trace names it (Account.User: an email, a login), never by a token; ""
// for a key or a provider without an account (#557).
func accountOf(p provider.Provider) string {
	if p.Account == nil {
		return ""
	}
	return p.Account.User
}

func appendUsage(r *http.Request, rec usage.Record) {
	if rec.Session == "" {
		rec.Session = sessionOf(r.Header)
	}
	if rec.Subagent == "" {
		rec.Subagent, rec.ParentAgent = subagentOf(r.Header)
	}
	who := access.Caller(r.Context())
	rec.CallerKeyID, rec.CallerKeyName = who.KeyID, who.KeyName
	rec.Local = local(r)
	if r.Context().Value(observedKey{}) != nil {
		rec.Watched = true
	}
	budget.Append(rec)
}

// lanKeyed marks a request from another machine that carried the key.
type lanKeyed struct{}

// sharedWith: the request came from another machine with the key, the
// gateway shared from the Settings page.
func sharedWith(r *http.Request) bool {
	ok, _ := r.Context().Value(lanKeyed{}).(bool)
	return ok
}

// callerKey is the API key a request carries, however its client sends one:
// the first of callerKeys, "" for none.
func callerKey(r *http.Request) string {
	if ks := callerKeys(r); len(ks) > 0 {
		return ks[0]
	}
	return ""
}

// callerKeys are the API keys a request carries, in the order they are
// read: Authorization's bearer token, x-api-key, x-goog-api-key, ?key=. A
// header that carries none ("Authorization: Bearer ", sent beside an
// x-api-key by a client with no token set) is passed over, and a client
// that sends two different values — a stale or placeholder token in
// Authorization (ANTHROPIC_AUTH_TOKEN left at magpie), the key in x-api-key
// — is let in by whichever is an enabled gateway key. Loopback takes any
// token, so this mattered only from another computer, which was refused
// what this one was answered (悠悠哥 on Discord: a client's model list from
// magpie shared on a NAS failed, from magpie on 127.0.0.1 it came).
func callerKeys(r *http.Request) []string {
	var out []string
	add := func(v string) {
		if v = strings.TrimSpace(v); v != "" && !slices.Contains(out, v) {
			out = append(out, v)
		}
	}
	a := strings.TrimSpace(r.Header.Get("Authorization"))
	if len(a) >= 7 && strings.EqualFold(a[:7], "Bearer ") {
		a = a[7:]
	} else if strings.EqualFold(a, "Bearer") {
		a = ""
	}
	add(a)
	for _, h := range []string{"x-api-key", "x-goog-api-key"} {
		add(r.Header.Get(h))
	}
	add(r.URL.Query().Get("key"))
	return out
}

// managedKey: the request carries a named gateway key's form (sk-magpie-…),
// or a key's own value a user gave it, in any of the places a key is read.
func managedKey(r *http.Request) bool {
	return slices.ContainsFunc(callerKeys(r), access.Named)
}

// refusedKey says why a caller's key was turned away: none came, or the
// one that came (its last four characters, and only of a long one) is no enabled
// gateway key, so a client that drops its key is told apart from a wrong
// key (#545).
func refusedKey(key string) string {
	if key == "" {
		return "no API key was sent: send a magpie gateway key as Authorization: Bearer <key> (or x-api-key)"
	}
	which := "the API key sent"
	if len(key) >= 12 { // a short one isn't named, so as not to give most of it away
		which = "the API key ending in " + key[len(key)-4:]
	}
	return which + " is not an enabled magpie gateway key: it is disabled, removed or mistyped"
}

// local is a request from this computer: one that reached the gateway on
// loopback and wasn't passed on by a proxy or tunnel running here.
func local(r *http.Request) bool {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		return false
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback() && !proxied(r)
}

// forwardHeaders are what a proxy or tunnel adds to a request it passes
// on: cloudflared (Cf-Connecting-IP, and X-Forwarded-For beside it),
// ngrok, Tailscale serve and funnel, frp's and Caddy's and nginx's HTTP
// proxies. An agent calling magpie sends none of them.
var forwardHeaders = []string{"Forwarded", "X-Forwarded-For", "X-Real-IP", "Cf-Connecting-IP", "True-Client-IP", "Tailscale-User-Login"}

// proxied: the request came to loopback through a proxy or tunnel on this
// computer (#1022, znjhahaha: a Cloudflare Tunnel to 127.0.0.1:3425), so it
// is someone else's, from wherever the proxy reaches, and is treated as
// from another machine: answered only while magpie is shared, with an
// enabled gateway key. MAGPIE_TRUST_PROXY=1 says a proxy here signs its
// clients in itself, and keeps what it forwards local, as before.
func proxied(r *http.Request) bool {
	if trustProxy() {
		return false
	}
	return slices.ContainsFunc(forwardHeaders, func(h string) bool { return r.Header.Get(h) != "" })
}

func trustProxy() bool {
	v, _ := strconv.ParseBool(strings.TrimSpace(os.Getenv("MAGPIE_TRUST_PROXY")))
	return v
}
