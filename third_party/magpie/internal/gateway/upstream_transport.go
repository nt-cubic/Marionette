package gateway

import (
	"crypto/tls"
	"net/http"
	"time"

	"github.com/yetone/magpie/internal/netproxy"
)

// upstreamTLS is what the gateway's transports dial TLS with: nil, the
// system's roots, but for a test trusting its own server.
var upstreamTLS *tls.Config

// upstreamTransport is the gateway's transport to the vendors: a provider
// with a proxy of its own is sent through a transport kept for that proxy
// (#237), and the ChatGPT backend over HTTP/1.1 (netproxy.HTTP1). Every
// other vendor is spoken HTTP/2 to wherever it offers it. A request that
// finds its kept-alive connection dropped is sent again on a new one
// (replayable). A vendor is given upstreamHeaderWait to begin answering,
// but for a drawing, which an images API answers only once it has drawn:
// it goes through a transport of its own that waits as long as Settings'
// Image generation timeout gives it (drawWithin), up to 60 minutes.
func upstreamTransport() http.RoundTripper {
	return drawsApart{
		any:   replayable{netproxy.Dispatch(vendorTransport(upstreamHeaderWait))},
		draws: replayable{netproxy.Dispatch(vendorTransport(0))},
	}
}

// upstreamHeaderWait is how long a vendor is given to begin answering a
// request that isn't a drawing.
var upstreamHeaderWait = 10 * time.Minute

// vendorTransport is a transport to the vendors that gives one wait to
// begin answering, none for 0: the request's context then bounds it.
func vendorTransport(wait time.Duration) *http.Transport {
	return &http.Transport{
		Proxy:                 netproxy.Func,
		ResponseHeaderTimeout: wait,
		MaxIdleConnsPerHost:   8,
		IdleConnTimeout:       90 * time.Second,
		ForceAttemptHTTP2:     true,
		TLSClientConfig:       upstreamTLS,
		// a silent HTTP/2 connection is pinged and dropped (#2843)
		HTTP2: netproxy.HTTP2Health(),
	}
}

// drawsApart sends a drawing's request (drawWaitOf) through draws, every
// other through any.
type drawsApart struct{ any, draws http.RoundTripper }

func (t drawsApart) RoundTrip(r *http.Request) (*http.Response, error) {
	if _, ok := drawWaitOf(r.Context()); ok {
		return t.draws.RoundTrip(r)
	}
	return t.any.RoundTrip(r)
}

// replayable lets Go's transport send a request again, at once and on a new
// connection, when the kept-alive connection it went out on turns out to
// be dropped: written into, and closed before a byte of a reply ("EOF",
// "write: broken pipe"). Go does so only for a request it may replay — a
// GET, or one marked idempotent — and only on a reused connection, so a
// POST to a vendor failed as a 502 and was asked again after a pause, each
// pause on a connection a proxy or the vendor's front had dropped while
// idle (#2136: Pi's Codex turn through Clash, "write: broken pipe" then
// "EOF", 14.5s before the third try). The mark is an X-Idempotency-Key with
// no value, which Go reads and never sends: the vendor sees the request as
// before. magpie already asked the same request again after these errors,
// so nothing is sent that wasn't before, only sooner.
type replayable struct{ http.RoundTripper }

func (t replayable) RoundTrip(req *http.Request) (*http.Response, error) {
	_, marked := req.Header["Idempotency-Key"]
	if _, ok := req.Header["X-Idempotency-Key"]; ok {
		marked = true
	}
	if marked || req.Method == http.MethodGet || req.Method == http.MethodHead ||
		(req.Body != nil && req.Body != http.NoBody && req.GetBody == nil) {
		return t.RoundTripper.RoundTrip(req)
	}
	// a copy, as a RoundTripper leaves its caller's request as it was
	r := req.Clone(req.Context())
	r.Header["X-Idempotency-Key"] = nil
	return t.RoundTripper.RoundTrip(r)
}
