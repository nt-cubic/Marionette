package gateway

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strings"
	"syscall"
	"time"

	"github.com/yetone/magpie/internal/netproxy"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A URL magpie didn't choose itself is downloaded only from public
// addresses: an image a caller names (an edit's, or one to draw from) for
// a vendor that takes only the bytes, an image or video a vendor answers
// with (its URL may come from what the model was steered to say), and a
// page web_fetch reads. Never from this computer (loopback), the user's
// own network (RFC 1918, fc00::/7), link-local addresses (169.254/16 with
// the cloud metadata service, fe80::/10), CGNAT (100.64/10, Tailscale's),
// or the other special ranges below — through any redirect too. Otherwise
// whoever can send magpie a request could have it read a service only
// this computer reaches and get its answer back as the "image".
//
// Where the request came from changes nothing: one that reaches magpie
// on loopback may well be a reverse proxy's or a relay's on the same
// machine, passing on someone else's. Two exceptions, both named by the
// user rather than guessed:
//   - settings.json's fetchPrivate: hosts (name or name:port), addresses
//     and CIDR ranges the user lets these downloads reach;
//   - a vendor's answer may name its own host and port (a provider the
//     user set up on localhost or their network serves its images there).
//
// How it holds:
//   - the host is resolved, and every address it resolves to must be
//     public; the request is then sent to the address checked (pinned),
//     with the host kept for the Host header and TLS, so a name that
//     resolves differently the second time (DNS rebinding) changes
//     nothing;
//   - a direct connection is checked again on the address actually
//     dialled (privateDialer's Control);
//   - through a proxy (magpie's, the provider's, the system's) the proxy
//     is dialled, and it is sent the pinned address, never the name, so
//     it can't resolve it to another (an https:// CONNECT, SOCKS, or a
//     plain http:// request whose Host is then the address too);
//   - each redirect goes through the same, at most imageRedirects of them.

// imageRedirects is how many redirects a caller's image URL is followed
// through.
const imageRedirects = 5

// errPrivateAddress is a URL that leads somewhere other than the public
// internet.
var errPrivateAddress = errors.New(`it is at a private address, which magpie doesn't download from: send the image itself (a data: URL or a file) instead, or name the host in settings.json's "fetchPrivate"`)

// fetchAllow is where a download may go beyond public addresses: the
// user's fetchPrivate, and the hosts a transport was made for.
type fetchAllow struct {
	hosts map[string]bool // "name" (any port) or "name:port", lower case
	nets  []netip.Prefix
}

// allowFor reads fetchPrivate, and adds also. A settings.json that can't
// be read is unknown, so it allows nothing of its own.
func allowFor(also []string) fetchAllow {
	a := fetchAllow{hosts: map[string]bool{}}
	var list []string
	if st, err := settings.LoadChecked(); err == nil {
		list = st.FetchPrivate
	}
	for _, e := range append(list, also...) {
		e = strings.ToLower(strings.TrimSpace(e))
		if e == "" {
			continue
		}
		if p, err := netip.ParsePrefix(e); err == nil {
			a.nets = append(a.nets, p.Masked())
		} else if ip, err := netip.ParseAddr(strings.Trim(e, "[]")); err == nil {
			a.nets = append(a.nets, netip.PrefixFrom(ip.Unmap(), ip.Unmap().BitLen()))
		} else {
			a.hosts[e] = true
		}
	}
	return a
}

// addr says a may be fetched from: public, or in a range the user named.
func (a fetchAllow) addr(ip netip.Addr) bool {
	if publicAddr(ip) {
		return true
	}
	for _, n := range a.nets {
		if n.Contains(ip.Unmap()) {
			return true
		}
	}
	return false
}

// host says host:port was named itself, so whatever it resolves to may be
// fetched from.
func (a fetchAllow) host(host, port string) bool {
	host = strings.ToLower(strings.Trim(host, "[]"))
	return a.hosts[host] || a.hosts[net.JoinHostPort(host, port)]
}

// hostOf is u's host and port as fetchAllow names it, "" for none.
func hostOf(u string) string {
	p, err := url.Parse(strings.TrimSpace(u))
	if err != nil || p.Hostname() == "" {
		return ""
	}
	port := p.Port()
	if port == "" {
		port = map[string]string{"http": "80", "https": "443"}[p.Scheme]
	}
	if port == "" {
		return ""
	}
	return net.JoinHostPort(strings.ToLower(p.Hostname()), port)
}

// vendorClient downloads what a vendor answered with by URL: from public
// addresses, and the hosts its own endpoints are at.
func vendorClient(p provider.Provider) *http.Client {
	var own []string
	for _, u := range []string{p.Chat, p.Responses, p.Anthropic, p.Gemini} {
		if h := hostOf(u); h != "" {
			own = append(own, h)
		}
	}
	return guardedClient(own...)
}

// lookupIP resolves a host for a caller's image; a test swaps it.
var lookupIP = func(ctx context.Context, host string) ([]netip.Addr, error) {
	return net.DefaultResolver.LookupNetIP(ctx, "ip", host)
}

// specialNets are ranges no caller's image is fetched from beyond those
// net/netip names (loopback, private, link-local, multicast, unspecified).
var specialNets = func() []netip.Prefix {
	var out []netip.Prefix
	for _, s := range []string{
		"0.0.0.0/8",       // "this" network
		"100.64.0.0/10",   // CGNAT, Tailscale
		"192.0.0.0/24",    // IETF protocol assignments
		"192.0.2.0/24",    // documentation
		"198.18.0.0/15",   // benchmarking
		"198.51.100.0/24", // documentation
		"203.0.113.0/24",  // documentation
		"240.0.0.0/4",     // reserved, broadcast
		"::/96",           // IPv4-compatible (deprecated)
		"100::/64",        // discard
		"2001::/32",       // Teredo
		"2001:db8::/32",   // documentation
		"fec0::/10",       // site-local (deprecated)
		"64:ff9b:1::/48",  // local-use NAT64
	} {
		out = append(out, netip.MustParsePrefix(s))
	}
	return out
}()

// publicAddr says a may be fetched a caller's image from.
func publicAddr(a netip.Addr) bool {
	a = a.Unmap() // ::ffff:127.0.0.1 is 127.0.0.1
	if !a.IsValid() || a.IsLoopback() || a.IsPrivate() || a.IsUnspecified() ||
		a.IsLinkLocalUnicast() || a.IsLinkLocalMulticast() || a.IsInterfaceLocalMulticast() || a.IsMulticast() {
		return false
	}
	for _, n := range specialNets {
		if n.Contains(a) {
			return false
		}
	}
	if a.Is6() {
		b := a.As16()
		// an IPv4 address inside an IPv6 one: NAT64 (64:ff9b::/96) and
		// 6to4 (2002::/16) reach it
		if netip.MustParsePrefix("64:ff9b::/96").Contains(a) {
			return publicAddr(netip.AddrFrom4([4]byte{b[12], b[13], b[14], b[15]}))
		}
		if b[0] == 0x20 && b[1] == 0x02 {
			return publicAddr(netip.AddrFrom4([4]byte{b[2], b[3], b[4], b[5]}))
		}
	}
	return true
}

// guardDialer dials only addresses ok allows, checked on the address the
// socket is about to connect to.
func guardDialer(ok func(netip.Addr) bool) *net.Dialer {
	return &net.Dialer{
		Timeout:   30 * time.Second,
		KeepAlive: 30 * time.Second,
		Control: func(_, address string, _ syscall.RawConn) error {
			ap, err := netip.ParseAddrPort(address)
			if err != nil || !ok(ap.Addr()) {
				return errPrivateAddress
			}
			return nil
		},
	}
}

// privateDialer dials only public addresses.
var privateDialer = guardDialer(publicAddr)

// publicTransport sends a request to a public address only (see above), or
// one fetchPrivate or also names.
type publicTransport struct{ also []string }

func (pt publicTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	if req.URL.Scheme != "http" && req.URL.Scheme != "https" {
		return nil, fmt.Errorf("%s isn't an http(s) URL", req.URL.Redacted())
	}
	host := req.URL.Hostname()
	if host == "" {
		return nil, errors.New("the URL names no host")
	}
	// the proxy this request would take, as any other of magpie's: chosen
	// by the name, before it is pinned to an address
	proxy, err := netproxy.Func(req)
	if err != nil {
		return nil, err
	}
	var addrs []netip.Addr
	if a, err := netip.ParseAddr(strings.TrimSuffix(strings.TrimPrefix(host, "["), "]")); err == nil {
		addrs = []netip.Addr{a}
	} else if addrs, err = lookupIP(req.Context(), host); err != nil {
		return nil, err
	}
	if len(addrs) == 0 {
		return nil, fmt.Errorf("%s resolves to no address", host)
	}
	port := req.URL.Port()
	if port == "" {
		port = map[string]string{"http": "80", "https": "443"}[req.URL.Scheme]
	}
	allow := allowFor(pt.also)
	named := allow.host(host, port)
	for _, a := range addrs {
		if !named && !allow.addr(a) {
			return nil, errPrivateAddress
		}
	}
	var tc *tls.Config
	if upstreamTLS != nil {
		tc = upstreamTLS.Clone()
	} else {
		tc = &tls.Config{}
	}
	tc.ServerName = host
	t := &http.Transport{
		Proxy:                 func(*http.Request) (*url.URL, error) { return proxy, nil },
		TLSClientConfig:       tc,
		ForceAttemptHTTP2:     true,
		DisableKeepAlives:     true,
		TLSHandshakeTimeout:   30 * time.Second,
		ResponseHeaderTimeout: 2 * time.Minute,
	}
	if proxy == nil {
		pinned := addrs[0].Unmap()
		t.DialContext = guardDialer(func(a netip.Addr) bool {
			return allow.addr(a) || named && a.Unmap() == pinned
		}).DialContext
	}
	out := req.Clone(req.Context())
	out.URL.Host = net.JoinHostPort(addrs[0].Unmap().String(), port)
	out.Host = req.Host
	if out.Host == "" {
		out.Host = req.URL.Host
	}
	// a plain http:// URL through an HTTP proxy is sent to it whole, its
	// host taken from Host: the proxy would resolve the name again, so it
	// is given the address instead (a site that serves only by name won't
	// answer it; https://, and any URL through SOCKS, keep the name)
	if proxy != nil && req.URL.Scheme == "http" && (proxy.Scheme == "http" || proxy.Scheme == "https") {
		out.Host = out.URL.Host
	}
	return t.RoundTrip(out)
}

// publicClient downloads a URL magpie didn't choose: from public
// addresses only (and fetchPrivate's), through at most imageRedirects
// redirects, each checked the same.
var publicClient = guardedClient()

// PublicClient is publicClient, for a download outside the gateway of a
// URL magpie didn't choose (generate_image's of a vendor's image).
func PublicClient() *http.Client { return publicClient }

// guardedClient is publicClient that also reaches also's hosts
// ("name:port").
func guardedClient(also ...string) *http.Client {
	return &http.Client{
		Transport: publicTransport{also: also},
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			if len(via) > imageRedirects {
				return fmt.Errorf("it redirects more than %d times", imageRedirects)
			}
			return nil
		},
	}
}
