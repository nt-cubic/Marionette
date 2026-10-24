package gateway

import (
	"context"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"strconv"
	"sync"
	"time"
)

// A restart to update hands the gateway over rather than ending it
// (internal/handoff): the magpie updating gives the new version the
// socket it listens on, the new one serves on it at once, and the old
// stops accepting and finishes the requests it has (handedOffDrain at
// most) before it quits. No connection is refused on the way: the socket
// is never closed, only accepted from by the other process.

// handedOffDrain is how long a magpie that handed the gateway over waits
// for its requests in flight to finish: the longest stream an agent has
// been seen to keep, as `make dev`'s handover waits.
var handedOffDrain = 15 * time.Minute

// shortDrain is how long the gateway's end waits for its requests when
// nothing serves the port after it: a quit, or a restart that waits.
var shortDrain = 2 * time.Second

// inherited is the listener the magpie before this one handed over, taken
// by the first Listen of its address; ack is called once this gateway
// serves on it, which tells the one before to stop accepting.
var inherited struct {
	sync.Mutex
	ln  net.Listener
	ack func()
}

// Inherit has this process's gateway serve on ln, the socket the magpie
// it replaces listened on, rather than listening itself; ack is called
// once it serves.
func Inherit(ln net.Listener, ack func()) {
	inherited.Lock()
	defer inherited.Unlock()
	if inherited.ln != nil {
		inherited.ln.Close()
	}
	inherited.ln, inherited.ack = ln, ack
}

// Inherited is whether a listener handed over is waiting to be served:
// the gateway's port is then held by the magpie that handed it over, and
// is this process's to serve all the same.
func Inherited() bool {
	inherited.Lock()
	defer inherited.Unlock()
	return inherited.ln != nil
}

// takeInherited is the listener handed over when it listens on addr, and
// the ack to call once it serves. One on another address (the settings
// changed in between) is closed: the port is then listened on as usual.
func takeInherited(addr string) (net.Listener, func()) {
	inherited.Lock()
	defer inherited.Unlock()
	ln, ack := inherited.ln, inherited.ack
	if ln == nil {
		return nil, nil
	}
	inherited.ln, inherited.ack = nil, nil
	if !sameAddr(ln.Addr(), addr) {
		log.Printf("gateway: the socket handed over listens on %s, not %s; listening anew", ln.Addr(), addr)
		ln.Close()
		return nil, nil
	}
	return ln, ack
}

// pendingAck is the ack of the socket handed over that Listen returned,
// for ListenAndServe to call once it serves on it.
var pendingAck struct {
	sync.Mutex
	ack func()
}

func setAck(ack func()) {
	pendingAck.Lock()
	pendingAck.ack = ack
	pendingAck.Unlock()
}

// takeAck is the ack waiting, nil when the listener wasn't handed over.
func takeAck() func() {
	pendingAck.Lock()
	defer pendingAck.Unlock()
	ack := pendingAck.ack
	pendingAck.ack = nil
	return ack
}

// sameAddr is whether a listens where addr names: the same port, on the
// same IP or both on every interface.
func sameAddr(a net.Addr, addr string) bool {
	ta, ok := a.(*net.TCPAddr)
	if !ok {
		return false
	}
	host, port, err := net.SplitHostPort(addr)
	if err != nil || port != strconv.Itoa(ta.Port) {
		return false
	}
	if host == "" {
		return ta.IP == nil || ta.IP.IsUnspecified()
	}
	ips := []net.IP{net.ParseIP(host)}
	if ips[0] == nil {
		// a name (localhost): any of its addresses
		var err error
		if ips, err = net.LookupIP(host); err != nil {
			return false
		}
	}
	for _, ip := range ips {
		if ip.IsUnspecified() {
			if ta.IP == nil || ta.IP.IsUnspecified() {
				return true
			}
		} else if ip.Equal(ta.IP) {
			return true
		}
	}
	return false
}

// ListenerFile is a copy of the socket the gateway listens on, to hand to
// the magpie that takes it over; the caller closes it once sent.
func (s *Server) ListenerFile() (*os.File, error) {
	s.lnMu.Lock()
	ln := s.ln
	s.lnMu.Unlock()
	if t, ok := ln.(*tlsListener); ok {
		ln = t.Listener
	}
	tl, ok := ln.(*net.TCPListener)
	if !ok {
		return nil, errors.New("the gateway isn't listening on a TCP socket")
	}
	return tl.File()
}

// HandOff has the gateway, once its ctx ends, wait for every request in
// flight to finish (handedOffDrain at most) rather than 2s: another
// magpie serves the port meanwhile.
func (s *Server) HandOff() { s.handedOff.Store(true) }

// drainFor is how long the gateway's end waits for the requests in flight.
func (s *Server) drainFor() time.Duration {
	if Handover || s.handedOff.Load() {
		return handedOffDrain
	}
	return shortDrain
}

// ServeCallbacks answers, on ln, the tool calls of the Claude Code runs
// this process started (claudeMCPPath): their MCP helper calls the
// gateway's port, which the magpie this one handed it to now serves, and
// that one passes on the calls of runs it doesn't have (ForwardCallbacks).
// It returns once ln is closed.
func (s *Server) ServeCallbacks(ln net.Listener) error {
	mux := http.NewServeMux()
	mux.HandleFunc("POST "+claudeMCPPath+"{token}", s.subscription.mcpCall)
	err := (&http.Server{Handler: mux, ReadHeaderTimeout: 30 * time.Second}).Serve(ln)
	if errors.Is(err, net.ErrClosed) || errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}

// predecessorCallbacks is the unix socket of the magpie this one took the
// gateway over from, which answers its own runs' tool calls there; ""
// when there is none.
var predecessorCallbacks struct {
	sync.Mutex
	sock string
}

// ForwardCallbacks has a tool call for a Claude Code run this process
// doesn't know passed on to sock, where the magpie it took the gateway
// over from answers those of its own runs (ServeCallbacks): a turn under
// way there when the gateway changed hands still gets its tool results.
// "" stops it.
func ForwardCallbacks(sock string) {
	predecessorCallbacks.Lock()
	predecessorCallbacks.sock = sock
	predecessorCallbacks.Unlock()
}

// forwardCallback passes the tool call r, for a run this process doesn't
// have, on to the magpie before this one; false when there is none to.
func forwardCallback(w http.ResponseWriter, r *http.Request) bool {
	predecessorCallbacks.Lock()
	sock := predecessorCallbacks.sock
	predecessorCallbacks.Unlock()
	if sock == "" {
		return false
	}
	if _, err := os.Stat(sock); err != nil {
		// that magpie has finished and gone
		ForwardCallbacks("")
		return false
	}
	p := &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(&url.URL{Scheme: "http", Host: "predecessor"})
			pr.Out.URL.Path, pr.Out.URL.RawPath = r.URL.Path, r.URL.RawPath
		},
		Transport: &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", sock)
		}},
		FlushInterval: -1,
		ErrorHandler: func(w http.ResponseWriter, _ *http.Request, err error) {
			if !errors.Is(err, io.EOF) {
				log.Printf("claude: a tool call for the magpie before this one didn't reach it: %v", err)
			}
			http.Error(w, "unknown or expired Claude run", http.StatusNotFound)
		},
	}
	p.ServeHTTP(w, r)
	return true
}

// forwarding is whether tool calls this process doesn't know go on to the
// magpie it took the gateway over from.
func forwarding() bool {
	predecessorCallbacks.Lock()
	sock := predecessorCallbacks.sock
	predecessorCallbacks.Unlock()
	return sock != ""
}

// Addr is where the gateway listens, "" before it does.
func (s *Server) Addr() string {
	s.lnMu.Lock()
	defer s.lnMu.Unlock()
	if s.ln == nil {
		return ""
	}
	return s.ln.Addr().String()
}
