package davsync

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// shortStall makes the watchdogs short for a test.
func shortStall(t *testing.T, stall, meta time.Duration) {
	s, m := stallAfter, metaLimit
	stallAfter, metaLimit = stall, meta
	t.Cleanup(func() { stallAfter, metaLimit = s, m })
}

// slowDAV is a WebDAV server on a slow line: the backup goes both ways in
// chunks of chunk bytes every gap, or stops after stopAt bytes for good. A
// PUT is answered tail after its body is all in: the time the last of it,
// handed over by magpie, would take to cross the line.
type slowDAV struct {
	chunk  int
	gap    time.Duration
	tail   time.Duration
	stopAt int           // 0: never
	quiet  chan struct{} // closed when the test is done with a stopped server
	mu     sync.Mutex
	stored []byte
}

// stop holds the request without a byte either way until the client gives
// up, or the test ends (a server doesn't see a client gone before it has
// read the body).
func (s *slowDAV) stop(r *http.Request) {
	select {
	case <-r.Context().Done():
	case <-s.quiet:
	}
}

func (s *slowDAV) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodPut:
		var got bytes.Buffer
		got.Grow(int(r.ContentLength))
		buf := make([]byte, s.chunk)
		for {
			if s.stopAt > 0 && got.Len() >= s.stopAt {
				s.stop(r) // the line went quiet
				return
			}
			n, err := io.ReadFull(r.Body, buf)
			got.Write(buf[:n])
			if err != nil {
				break
			}
			time.Sleep(s.gap)
		}
		time.Sleep(s.tail)
		s.mu.Lock()
		s.stored = got.Bytes()
		s.mu.Unlock()
		w.Header().Set("ETag", `"v1"`)
		w.WriteHeader(http.StatusCreated)
	case http.MethodHead:
		s.mu.Lock()
		w.Header().Set("Content-Length", strconv.Itoa(len(s.stored)))
		s.mu.Unlock()
		w.WriteHeader(http.StatusOK)
	case http.MethodGet:
		s.mu.Lock()
		data := s.stored
		s.mu.Unlock()
		w.Header().Set("Content-Length", strconv.Itoa(len(data)))
		w.Header().Set("ETag", `"v1"`)
		w.WriteHeader(http.StatusOK)
		for i := 0; i < len(data); i += s.chunk {
			if s.stopAt > 0 && i >= s.stopAt {
				s.stop(r)
				return
			}
			w.Write(data[i:min(i+s.chunk, len(data))])
			w.(http.Flusher).Flush()
			time.Sleep(s.gap)
		}
	default:
		w.WriteHeader(http.StatusMethodNotAllowed)
	}
}

// narrowLine serves h, and gives the client that reaches it, with the
// line's buffers held small. The system hands a writer room in bursts: Linux wakes it once a third
// of its send buffer has drained and opens the receiver's window by as much,
// and with the MBs it tunes both up to on a fast loopback, a body moving at
// 6.4 MB/s went 150–290 ms between two of magpie's writes, which a test's
// watchdog of 300 ms didn't always outlast though the line never stopped (at
// the real 60 s that is a line under ~25 KB/s). Small buffers keep the
// bursts to a chunk or two; what sits in buffers after the body is handed
// over is the server's tail.
func narrowLine(t *testing.T, h http.Handler) (*httptest.Server, *http.Client) {
	const buffer = 128 << 10
	srv := httptest.NewUnstartedServer(h)
	srv.Listener = narrowListener{srv.Listener, buffer}
	srv.Start()
	t.Cleanup(srv.Close)
	tr := http.DefaultTransport.(*http.Transport).Clone()
	dial := tr.DialContext
	tr.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
		c, err := dial(ctx, network, addr)
		if err == nil {
			err = narrow(c, buffer)
		}
		return c, err
	}
	t.Cleanup(tr.CloseIdleConnections)
	return srv, &http.Client{Transport: stallTransport{tr}}
}

type narrowListener struct {
	net.Listener
	buffer int
}

func (l narrowListener) Accept() (net.Conn, error) {
	c, err := l.Listener.Accept()
	if err == nil {
		err = narrow(c, l.buffer)
	}
	return c, err
}

// narrow keeps both ways of a connection small: the PUT goes up through the
// client's send buffer and the server's receive buffer, the GET comes down
// through the other two, and on macOS a download through the tuned-up pair
// burst as the upload had (run 37693090857: "took nothing for 300ms (GET)").
func narrow(c net.Conn, buffer int) error {
	tc := c.(*net.TCPConn)
	if err := tc.SetReadBuffer(buffer); err != nil {
		return err
	}
	return tc.SetWriteBuffer(buffer)
}

// A backup that takes far longer than the watchdog to go up and come back
// down, but never stops moving, is written and read in full (#657: 43 MB
// at 305 KB/s took 2.4 minutes, and a sync was given 2).
func TestSlowTransferIsNotCutShort(t *testing.T) {
	// a second, not the 300 ms the other stall tests use: a macOS runner
	// can stop the whole test for a few hundred ms, and here the line never
	// stops, so such a pause is all a shorter watchdog would catch (red
	// on macOS in run 37693090857, near the end of the GET)
	shortStall(t, time.Second, time.Second)
	// the PUT's last bytes cross the line twice the watchdog after magpie
	// has handed them all over
	s := &slowDAV{chunk: 64 << 10, gap: 125 * time.Millisecond, tail: 2 * stallAfter}
	srv, client := narrowLine(t, s)
	d, err := newDAV(Config{URL: srv.URL})
	if err != nil {
		t.Fatal(err)
	}
	d.client = client
	// 2 MB at 512 KB/s: about 4 s each way. Not more: the read back fills
	// fresh buffers half as big as what has come, the watchdog counts that
	// time too, and beside a full suite on a Linux box a 24 MB backup's GET
	// went off ("took nothing for 1s (GET)") though the line never stopped.
	data := bytes.Repeat([]byte("magpie skill "), (2<<20)/13)
	ctx := context.Background()
	start := time.Now()
	if _, err := d.put(ctx, data, ""); err != nil {
		t.Fatalf("slow upload: %v", err)
	}
	up := time.Since(start)
	start = time.Now()
	got, _, err := d.get(ctx, version{})
	if err != nil {
		t.Fatalf("slow download: %v", err)
	}
	down := time.Since(start)
	if !bytes.Equal(got, data) {
		t.Fatalf("read back %d bytes of %d", len(got), len(data))
	}
	if up < 3*stallAfter || down < 3*stallAfter {
		t.Fatalf("the transfers (%s up, %s down) weren't slow enough to test anything", up, down)
	}
}

// A server that stops in the middle, either way, fails the sync soon after
// it stops, saying so — not after however long a sync may take.
func TestStalledTransferFailsPromptly(t *testing.T) {
	shortStall(t, 300*time.Millisecond, 300*time.Millisecond)
	data := bytes.Repeat([]byte("x"), 32<<20) // more than the system buffers hold
	for _, way := range []string{"up", "down"} {
		t.Run(way, func(t *testing.T) {
			s := &slowDAV{chunk: 64 << 10, gap: 10 * time.Millisecond, quiet: make(chan struct{})}
			if way == "up" {
				s.stopAt = 1 << 20
			} else {
				s.stored = data
				s.stopAt = 1 << 20
			}
			srv := httptest.NewServer(s)
			defer srv.Close()
			defer close(s.quiet)
			d, err := newDAV(Config{URL: srv.URL})
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			start := time.Now()
			if way == "up" {
				_, err = d.put(ctx, data, "")
			} else {
				_, _, err = d.get(ctx, version{})
			}
			took := time.Since(start)
			var st *errStalled
			if !errors.As(err, &st) {
				t.Fatalf("%v after %s, want a stall", err, took)
			}
			if took > 10*time.Second {
				t.Fatalf("a stall took %s to tell", took)
			}
		})
	}
}

// A request that moves only a little (PROPFIND, MKCOL…) has metaLimit in
// all, however it trickles.
func TestMetadataRequestHasALimit(t *testing.T) {
	shortStall(t, 300*time.Millisecond, 300*time.Millisecond)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-time.After(5 * time.Second):
		case <-r.Context().Done():
		}
	}))
	defer srv.Close()
	d, err := newDAV(Config{URL: srv.URL})
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	err = d.mkcol(context.Background())
	var st *errStalled
	if !errors.As(err, &st) || time.Since(start) > 3*time.Second {
		t.Fatalf("%v after %s, want the limit", err, time.Since(start))
	}
}

// slowGitHub is the least of GitHub's API a commit talks to, so a blob POST
// can be eaten as slowly as a live line: chunk bytes every gap. GitHub is
// the one backend whose uploads ride a method the watchdog used to see as
// metadata: its blob POSTs haul up to 16 MiB of file each, about 21 MB of
// base64 (#4576).
type slowGitHub struct {
	chunk     int
	gap       time.Duration
	hangTrees bool // sit on the tree POST for good, as a server that stopped
}

func (h *slowGitHub) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	send := func(status int, v any) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		json.NewEncoder(w).Encode(v)
	}
	base := "/repos/alice/private"
	switch {
	case r.URL.Path == base:
		send(http.StatusOK, map[string]any{"default_branch": "main", "private": true})
	case strings.HasSuffix(r.URL.Path, "/git/ref/heads/main"):
		send(http.StatusOK, map[string]any{"object": map[string]string{"type": "commit", "sha": "c1"}})
	case strings.Contains(r.URL.Path, "/contents/"):
		send(http.StatusNotFound, map[string]string{"message": "Not Found"})
	case r.URL.Path == base+"/git/blobs" && r.Method == http.MethodPost:
		var got bytes.Buffer
		buf := make([]byte, max(h.chunk, 1))
		for {
			n, err := r.Body.Read(buf)
			got.Write(buf[:n])
			if err != nil {
				break
			}
			time.Sleep(h.gap)
		}
		var in struct{ Content, Encoding string }
		if json.Unmarshal(got.Bytes(), &in) != nil || in.Encoding != "base64" {
			send(http.StatusUnprocessableEntity, map[string]string{"message": "Invalid blob"})
			return
		}
		b, err := base64.StdEncoding.DecodeString(in.Content)
		if err != nil {
			send(http.StatusUnprocessableEntity, map[string]string{"message": "Invalid blob"})
			return
		}
		send(http.StatusCreated, map[string]string{"sha": gitBlobSHA(b)})
	case strings.Contains(r.URL.Path, "/git/commits/"):
		send(http.StatusOK, map[string]any{"tree": map[string]string{"sha": "t1"}})
	case r.URL.Path == base+"/git/trees" && r.Method == http.MethodPost:
		if h.hangTrees {
			// the body has to go for the server to see the client gone
			io.Copy(io.Discard, r.Body)
			select {
			case <-time.After(5 * time.Second):
			case <-r.Context().Done():
			}
			return
		}
		send(http.StatusOK, map[string]string{"sha": "t2"})
	case r.URL.Path == base+"/git/commits" && r.Method == http.MethodPost:
		send(http.StatusOK, map[string]string{"sha": "c2"})
	case strings.HasSuffix(r.URL.Path, "/git/refs/heads/main") && r.Method == http.MethodPatch:
		send(http.StatusOK, map[string]string{})
	default:
		send(http.StatusNotFound, map[string]string{"message": "Not Found"})
	}
}

// githubOn wraps a test GitHub server the way the app reaches GitHub: the
// API's host rewritten onto the test server, over the stall watchdog (and
// the line's small buffers, as narrowLine holds them).
func githubOn(t *testing.T, h http.Handler) *github {
	t.Helper()
	srv, client := narrowLine(t, h)
	target, err := url.Parse(srv.URL)
	if err != nil {
		t.Fatal(err)
	}
	g, err := newGitHub(Config{URL: "github://alice/private/team", Password: "tok"})
	if err != nil {
		t.Fatal(err)
	}
	g.client = &http.Client{Transport: githubTransport{client.Transport, target}}
	return g
}

// A GitHub blob POST whose body goes up on a live but slow line is not cut
// short: a flat 30 s killed the whole sync, for good, on any line that
// couldn't push a 16 MiB part's 21 MB of base64 through it in half a
// minute (#4576: the sync stopped at 10:30 and every attempt after died on
// the same POST).
func TestGitHubSlowBlobUploadIsNotCutShort(t *testing.T) {
	shortStall(t, time.Second, 300*time.Millisecond)
	h := &slowGitHub{chunk: 16 << 10, gap: 50 * time.Millisecond}
	g := githubOn(t, h)
	// 200 KB of file: about 272 KB of base64 body, ~0.9 s at the line's
	// 320 KB/s, well over the whole metadata limit.
	data := bytes.Repeat([]byte("magpie usage "), (200<<10)/13)
	if _, err := g.put(context.Background(), data, ""); err != nil {
		t.Fatalf("a slow but live blob POST: %v", err)
	}
}

// A GitHub POST that hauls only a little (the tree, the commit, the ref —
// kilobytes at most) keeps metaLimit in all, however long the server sits
// on it: the big-body budget isn't a pass for every request.
func TestGitHubSmallPostKeepsTheLimit(t *testing.T) {
	shortStall(t, 300*time.Millisecond, 300*time.Millisecond)
	g := githubOn(t, &slowGitHub{hangTrees: true})
	start := time.Now()
	_, err := g.put(context.Background(), []byte("sealed fixture"), "")
	took := time.Since(start)
	var st *errStalled
	if !errors.As(err, &st) || st.idle || took > 3*time.Second {
		t.Fatalf("%v after %s, want the metadata limit on a small POST", err, took)
	}
}

// A switch of the global proxy leaves the sync riding the old one: the
// sync goes over http.DefaultTransport, whose idle connections only a
// DefaultClient request would ever drop (#4824). The sync's own request
// sees the move, drops them, and dials through the proxy in force.
func TestGlobalSwitchDialsTheSyncThroughTheNewProxy(t *testing.T) {
	target := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "dav") }))
	target.EnableHTTP2 = true
	target.StartTLS()
	t.Cleanup(target.Close)
	pool := x509.NewCertPool()
	pool.AddCert(target.Certificate())
	a, na := fakeSOCKS(t)
	b, nb := fakeSOCKS(t)
	cur := atomic.Pointer[url.URL]{}
	cur.Store(a)
	tr := http.DefaultTransport.(*http.Transport)
	oldProxy := tr.Proxy
	cfg := tr.TLSClientConfig
	wasNil := cfg == nil
	if wasNil {
		cfg = &tls.Config{}
		tr.TLSClientConfig = cfg
	}
	oldRoots := cfg.RootCAs
	cfg.RootCAs = pool
	t.Cleanup(func() {
		tr.Proxy = oldProxy
		cfg.RootCAs = oldRoots
		if wasNil {
			tr.TLSClientConfig = nil
		}
	})
	tr.Proxy = func(*http.Request) (*url.URL, error) { return cur.Load(), nil }
	// the proxies saved below are dead once the test ends: the settings
	// go back to what they were for the tests after it
	was := settings.Load()
	t.Cleanup(func() { settings.Save(was) })
	get := func() {
		res, err := syncClient.Get(target.URL)
		if err != nil {
			t.Fatalf("the sync: %v", err)
		}
		io.Copy(io.Discard, res.Body)
		res.Body.Close()
		if res.ProtoMajor != 2 {
			t.Fatalf("the sync went over %s, want HTTP/2", res.Proto)
		}
	}
	if err := settings.Save(settings.Settings{Proxy: a.String()}); err != nil {
		t.Fatal(err)
	}
	get()
	if na.Load() != 1 || nb.Load() != 0 {
		t.Fatalf("the first sync: %d connections through A, %d through B", na.Load(), nb.Load())
	}
	cur.Store(b)
	if err := settings.Save(settings.Settings{Proxy: b.String()}); err != nil {
		t.Fatal(err)
	}
	get()
	if nb.Load() != 1 || na.Load() != 1 {
		t.Fatalf("after the switch: %d connections through A, %d through B, want the sync to dial through B and no new one through A", na.Load(), nb.Load())
	}
}

// fakeSOCKS is a SOCKS5 proxy counting the connections it carried.
func fakeSOCKS(t *testing.T) (*url.URL, *atomic.Int32) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	var n atomic.Int32
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go func(c net.Conn) {
				defer c.Close()
				var h [2]byte
				io.ReadFull(c, h[:])
				io.CopyN(io.Discard, c, int64(h[1]))
				c.Write([]byte{5, 0})
				var r [4]byte
				io.ReadFull(c, r[:])
				var host string
				switch r[3] {
				case 1:
					b := make([]byte, 4)
					io.ReadFull(c, b)
					host = net.IP(b).String()
				case 3:
					var l [1]byte
					io.ReadFull(c, l[:])
					b := make([]byte, l[0])
					io.ReadFull(c, b)
					host = string(b)
				}
				var pb [2]byte
				io.ReadFull(c, pb[:])
				up, err := net.Dial("tcp", net.JoinHostPort(host, strconv.Itoa(int(binary.BigEndian.Uint16(pb[:])))))
				if err != nil {
					c.Write([]byte{5, 5, 0, 1, 0, 0, 0, 0, 0, 0})
					return
				}
				defer up.Close()
				n.Add(1)
				c.Write([]byte{5, 0, 0, 1, 127, 0, 0, 1, 0, 0})
				go io.Copy(up, c)
				io.Copy(c, up)
			}(c)
		}
	}()
	return &url.URL{Scheme: "socks5", Host: ln.Addr().String()}, &n
}
