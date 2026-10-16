package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// A plugin's second account keeps the queue set on its provider, as the
// first does (#1899, WorkBuddy AI): with one request at once on it, a
// queue of one and a wait of a second, a third request is turned away at
// once and the one queued after a second, never waiting on until its
// agent gives up (62s in the report).
func TestPluginSecondAccountKeepsTheQueue(t *testing.T) {
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)

	var mu sync.Mutex
	var seen []string
	gate := make(chan struct{})
	let := sync.OnceFunc(func() { close(gate) })
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		mu.Lock()
		seen = append(seen, r.Header.Get("Authorization"))
		mu.Unlock()
		select {
		case <-gate:
		case <-r.Context().Done():
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	t.Cleanup(up.Close)
	t.Cleanup(let) // runs before up.Close, which waits for the held request
	t.Setenv("FAKE_BASE", up.URL+"/v1")

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	for _, k := range []string{"k1", "k2"} {
		if _, err := provider.PluginAPIKey(ctx, "fakeco", 0, nil, k); err != nil {
			t.Fatal(err)
		}
	}
	ls := provider.Logins("fakeco")
	if len(ls) != 2 || !ls[1].On || ls[1].Active {
		t.Fatalf("accounts %+v, want a second one on beside the first", ls)
	}
	second := ls[1].User
	// as the report has it: each account's own limit, and the provider's
	// queue
	one := 1
	for _, l := range ls {
		if err := provider.SetAccountConcurrency("fakeco", l.User, &one); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SetQueue("fakeco", 1, 1); err != nil {
		t.Fatal(err)
	}
	p, err := provider.Find("fakeco")
	if err != nil {
		t.Fatal(err)
	}
	also := p.AlsoOn()
	if len(also) != 1 {
		t.Fatalf("%d accounts on beside the first, want 1", len(also))
	}
	if q := also[0]; q.QueueLimit != 1 || q.QueueWait != 1 {
		t.Fatalf("the second account's queue = %d, wait %ds; want the provider's 1, 1s", q.QueueLimit, q.QueueWait)
	}

	s := New()
	gw := httptest.NewServer(s.Handler())
	t.Cleanup(gw.Close)
	type reply struct {
		tag, body string
		status    int
		took      time.Duration
	}
	replies := make(chan reply, 3)
	send := func(tag string) {
		began := time.Now()
		go func() {
			req, _ := http.NewRequestWithContext(ctx, "POST", gw.URL+"/v1/chat/completions",
				strings.NewReader(`{"model":"fakeco/fake-1","stream":true,"messages":[{"role":"user","content":"`+tag+`"}]}`))
			req.Header.Set("Content-Type", "application/json")
			req.Header.Set(AccountHeader, second)
			res, err := http.DefaultClient.Do(req)
			if err != nil {
				replies <- reply{tag: tag, body: err.Error(), took: time.Since(began)}
				return
			}
			b, _ := io.ReadAll(res.Body)
			res.Body.Close()
			replies <- reply{tag, string(b), res.StatusCode, time.Since(began)}
		}()
	}
	next := func() reply {
		t.Helper()
		select {
		case r := <-replies:
			return r
		case <-time.After(10 * time.Second):
			t.Fatal("no reply in 10s: a request is still waiting in the second account's queue, its wait of 1s not kept")
		}
		return reply{}
	}
	busy := func(n, waiting int) func() bool {
		return func() bool {
			for _, l := range s.Lanes() {
				if l.Busy == n && l.Waiting == waiting {
					return true
				}
			}
			return false
		}
	}

	send("a")
	within(t, "a out on the second account", busy(1, 0))
	send("b")
	within(t, "b queued behind it", busy(1, 1))
	// the queue holds one: c is turned away at once
	send("c")
	r := next()
	if r.tag != "c" || r.status != 429 || !strings.Contains(r.body, "queue is full") {
		t.Fatalf("c = %+v, want an immediate 429 saying the queue is full", r)
	}
	// b waits its second, and is turned away
	r = next()
	if r.tag != "b" || r.status != 429 || !strings.Contains(r.body, "waited") {
		t.Fatalf("b = %+v, want a 429 for waiting as long as it may", r)
	}
	let()
	if r := next(); r.tag != "a" || r.status != 200 || !strings.Contains(r.body, "ok") {
		t.Fatalf("a = %+v", r)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(seen) != 1 || seen[0] != "Bearer k2" {
		t.Fatalf("the vendor saw %v, want a alone, on the second account", seen)
	}
}
