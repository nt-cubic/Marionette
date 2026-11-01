package plugin

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// gatedDist serves the dist branch with pkg's versions, its index.json
// answered as fail says: 503 while it is true, and only once open is
// closed. Its address is distBase for the test.
func gatedDist(t *testing.T, pkg string, versions []string, fail *atomic.Bool) (open func()) {
	t.Helper()
	fakeDist(t, nil, false)
	inner := distServer(t, map[string][]string{pkg: versions}, false)
	gate := make(chan struct{})
	var once sync.Once
	open = func() { once.Do(func() { close(gate) }) }
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/index.json" {
			select {
			case <-gate:
			case <-r.Context().Done():
				return
			}
			if fail != nil && fail.Load() {
				http.Error(w, "unavailable", http.StatusServiceUnavailable)
				return
			}
		}
		res, err := http.Get(inner + r.URL.Path)
		if err != nil {
			http.Error(w, err.Error(), http.StatusBadGateway)
			return
		}
		defer res.Body.Close()
		w.WriteHeader(res.StatusCode)
		io.Copy(w, res.Body)
	}))
	t.Cleanup(srv.Close)
	t.Cleanup(func() { open(); distSettled(t) })
	was := distMirror
	distBase = srv.URL
	distMirror = func(string) string { return "" }
	t.Cleanup(func() { distMirror = was })
	return open
}

// A package only the dist branch has (the Chat page, @magpie-community/
// page-chat, never on npm) whose index.json comes slower than one
// package's ask (npmEach) isn't "not on npm" for the next hour: it is
// unknown, kept as nothing, and once the read that went on ends, the
// next ask has it (ferock on Discord, v0.1.2399: Coming soon).
func TestDistReadSlowerThanAnAskIsntMissing(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/page-chat"
	npmHasNothing(t)
	open := gatedDist(t, pkg, []string{"0.1.6", "0.1.7"}, nil)
	npmEach, npmAll = 200*time.Millisecond, time.Second

	if info, ok := Info(context.Background(), []string{pkg})[pkg]; ok {
		t.Fatalf("with the dist branch not read yet the market says %+v; want it unknown", info)
	}
	npmMu.Lock()
	e, kept := npmCached()[pkg]
	npmMu.Unlock()
	if kept {
		t.Fatalf("kept %+v as what npm said, with the dist branch unread", e)
	}
	c, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	_, err := npmAsk(c, pkg)
	cancel()
	if errors.Is(err, errNotFound) || !errors.Is(err, errDistUnread) {
		t.Fatalf("npmAsk with the dist branch unread: %v; want errDistUnread, not errNotFound", err)
	}

	// the read wasn't cut off with the ask: it ends once the branch
	// answers, and the next ask has its 0.1.7
	open()
	distSettled(t)
	if info := Info(context.Background(), []string{pkg})[pkg]; info.Version != "0.1.7" {
		t.Fatalf("once the dist branch answered the market says %+v; want its 0.1.7", info)
	}
}

// A dist branch that answers with an error is unknown too, never "not on
// npm", and is asked again once distWait has passed.
func TestDistReadFailedIsntMissing(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/page-chat"
	npmHasNothing(t)
	var fail atomic.Bool
	fail.Store(true)
	open := gatedDist(t, pkg, []string{"0.1.7"}, &fail)
	open()
	wait := distWait
	distWait = 0
	t.Cleanup(func() { distWait = wait })
	npmEach, npmAll = 5*time.Second, 10*time.Second

	if info, ok := Info(context.Background(), []string{pkg})[pkg]; ok {
		t.Fatalf("with the dist branch failing the market says %+v; want it unknown", info)
	}
	if _, err := npmAsk(context.Background(), pkg); errors.Is(err, errNotFound) {
		t.Fatalf("npmAsk with the dist branch failing: %v, said missing", err)
	}
	fail.Store(false)
	if info := Info(context.Background(), []string{pkg})[pkg]; info.Version != "0.1.7" {
		t.Fatalf("once the dist branch answered the market says %+v; want its 0.1.7", info)
	}
	// and a branch read that has no such package is missing, as before
	const gone = "@magpie-community/opencode-nothing-auth"
	if _, err := npmAsk(context.Background(), gone); !errors.Is(err, errNotFound) {
		t.Fatalf("npmAsk of a package neither has: %v; want errNotFound", err)
	}
}

// A "not on npm" an older build kept (one that took a cut-off dist read
// for the branch having nothing) is unknown once it is an hour old and
// the branch still can't be read: neither the listings (InfoCached) nor
// Info give it, so the page offers Install rather than Coming soon
// (zhudabao8433 on Discord: PowerContext, installable the day before).
// One said within the hour still stands.
func TestStaleMissingIsUnknownWhileDistUnread(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/gateway-powercontext"
	const fresh = "@magpie-community/opencode-nothing-auth"
	npmHasNothing(t)
	var fail atomic.Bool
	fail.Store(true)
	open := gatedDist(t, pkg, []string{"0.2.0"}, &fail)
	open()
	wait := distWait
	distWait = 0
	t.Cleanup(func() { distWait = wait })
	npmEach, npmAll = 5*time.Second, 10*time.Second

	npmMu.Lock()
	npmCached()[pkg] = npmEntry{NPM{}, time.Now().Add(-2 * time.Hour)}
	npmCached()[fresh] = npmEntry{NPM{}, time.Now().Add(-time.Minute)}
	npmMu.Unlock()

	if info, ok := InfoCached([]string{pkg})[pkg]; ok {
		t.Fatalf("listings give an hours-old \"not on npm\" as %+v; want it unknown", info)
	}
	if info, ok := Info(context.Background(), []string{pkg})[pkg]; ok {
		t.Fatalf("with the dist branch failing the market says %+v from an hours-old answer; want it unknown", info)
	}
	if _, ok := InfoCached([]string{fresh})[fresh]; !ok {
		t.Fatal("a \"not on npm\" said a minute ago was dropped; it stands for the hour")
	}
	fail.Store(false)
	distSettled(t)
	if info := Info(context.Background(), []string{pkg})[pkg]; info.Version != "0.2.0" {
		t.Fatalf("once the dist branch answered the market says %+v; want its 0.2.0", info)
	}
}
