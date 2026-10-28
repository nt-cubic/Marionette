package gateway

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// #3890: 192 streams at once through one plugin. The plugin host holds 128
// streams' windows; the rest used to be refused locally at once ("too many
// plugin calls are in flight"), each refusal a 502 try the gateway sent
// again. Now they wait for a window: every request is answered on its
// first try, and the vendor sees each once.
func TestPluginStreamsPastTheBudgetWait(t *testing.T) {
	const n = 192
	var hits, live, most atomic.Int64
	release := make(chan struct{})
	pid := besideFake(t, "stream-admit", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		hits.Add(1)
		now := live.Add(1)
		defer live.Add(-1)
		for m := most.Load(); now > m && !most.CompareAndSwap(m, now); m = most.Load() {
		}
		w.Header().Set("Content-Type", "text/event-stream")
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		select {
		case <-release:
		case <-r.Context().Done():
			return
		}
		io.WriteString(w, `data: {"id":"a","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}`+"\n\n")
		io.WriteString(w, `data: {"id":"a","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`+"\n\n")
		io.WriteString(w, "data: [DONE]\n\n")
	}))
	s := New()
	// macOS queues at most 128 connections for a listener to accept
	// (kern.ipc.somaxconn) and resets a dial past that (#3946): a dial
	// takes a place in pending until the gateway accepts its connection,
	// so no more than 64 of the 192 are dialing or queued at once. Giving
	// a place back never waits: the accept runs in the server's accept
	// loop under httptest's lock, and a dial that failed after its
	// handshake may have been accepted already
	pending := make(chan struct{}, 64)
	free := func() {
		select {
		case <-pending:
		default:
		}
	}
	gw := httptest.NewUnstartedServer(s.Handler())
	gw.Config.ConnState = func(_ net.Conn, st http.ConnState) {
		if st == http.StateNew {
			free()
		}
	}
	gw.Start()
	t.Cleanup(gw.Close)
	tr := http.DefaultTransport.(*http.Transport).Clone()
	tr.Proxy, tr.MaxIdleConnsPerHost = nil, n
	dial := tr.DialContext
	tr.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
		select {
		case pending <- struct{}{}:
		case <-ctx.Done():
			return nil, ctx.Err()
		}
		c, err := dial(ctx, network, addr)
		if err != nil {
			free()
		}
		return c, err
	}
	t.Cleanup(tr.CloseIdleConnections)
	client := &http.Client{Transport: tr, Timeout: 2 * time.Minute}

	model := pid + "/fake-1"
	var wg sync.WaitGroup
	errs := make(chan string, n)
	for range n {
		wg.Add(1)
		go func() {
			defer wg.Done()
			res, err := client.Post(gw.URL+"/v1/chat/completions", "application/json", strings.NewReader(pluginReplyRequest(model, true)))
			if err != nil {
				errs <- err.Error()
				return
			}
			b, _ := io.ReadAll(res.Body)
			res.Body.Close()
			if res.StatusCode != 200 || !strings.Contains(string(b), "ok") {
				errs <- res.Status + " " + string(b)
			}
		}()
	}
	// the host's windows are all held by streams the vendor keeps open
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	for live.Load() < 128 && ctx.Err() == nil {
		time.Sleep(10 * time.Millisecond)
	}
	if live.Load() < 128 {
		t.Fatalf("the vendor holds %d streams, want the host's 128", live.Load())
	}
	time.Sleep(300 * time.Millisecond) // the other 64 reach the host meanwhile
	close(release)
	wg.Wait()
	close(errs)
	for e := range errs {
		t.Error(e)
	}
	if h := hits.Load(); h != n {
		t.Errorf("the vendor saw %d requests, want %d", h, n)
	}
	if m := most.Load(); m > 128 {
		t.Errorf("the vendor held %d streams at once, past the host's 128", m)
	}
	routes := routesOf(s, model)
	if len(routes) == 0 {
		t.Fatal("no routes traced")
	}
	for _, r := range routes {
		if len(r.Tries) != 1 || r.Tries[0].Status != 200 {
			for _, tr := range r.Tries {
				t.Errorf("route %d: a try %d %s", r.ID, tr.Status, tr.Error)
			}
		}
	}
}
