package gateway

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// unroutedLimits sets when a request held before routing shows, and when
// it is answered for.
func unroutedLimits(t *testing.T, show, most time.Duration) {
	t.Helper()
	s, m := unroutedShow, unroutedMost
	unroutedShow, unroutedMost = show, most
	t.Cleanup(func() { unroutedShow, unroutedMost = s, m })
}

// heldSight holds pastedImage before it is routed: its image's
// description is in flight and never ends, as a describe stuck on
// something that never lets go would be. What it holds the request on is
// returned, closed to let it go with text.
func heldSight(t *testing.T, s *Server) (release func(text string)) {
	t.Helper()
	model, ok := seer()
	if !ok {
		t.Fatal("no model to describe images with")
	}
	sum := sha256.Sum256([]byte(model + "\x00" + visionEffort(model) + "\x00" + "data:image/png;base64,aGVsbG8="))
	key := hex.EncodeToString(sum[:])
	e := &sight{done: make(chan struct{})}
	s.sightMu.Lock()
	if s.sights == nil {
		s.sights = map[string]*sight{}
	}
	s.sights[key] = e
	s.sightOrder = append(s.sightOrder, key)
	s.sightMu.Unlock()
	closed := false
	t.Cleanup(func() {
		if !closed {
			close(e.done)
		}
	})
	return func(text string) {
		e.text = text
		closed = true
		close(e.done)
	}
}

// routesOf is the trace's routes for model now.
func routesOf(s *Server, model string) []Route {
	var out []Route
	for _, r := range s.Trace(context.Background(), 0, 0).Routes {
		if r.Model == model {
			out = append(out, r)
		}
	}
	return out
}

// heyusaku6 on X: Codex thought for over ten minutes on a model served
// through magpie, the Routing page showed nothing, and only restarting
// both ended it. A request held before any route is begun for it shows on
// the Routing page as routing, and is answered with an error in bounded
// time, whole, rather than left to wait with nothing said.
func TestUnroutedRequestShownThenAnswered(t *testing.T) {
	unroutedLimits(t, 100*time.Millisecond, time.Second)
	s, _ := slowEyed(t, 0)
	heldSight(t, s)
	srv := httptest.NewServer(s.Handler())
	t.Cleanup(srv.Close)

	type reply struct {
		code int
		body string
		err  error
	}
	got := make(chan reply, 1)
	go func() {
		c := &http.Client{Timeout: 20 * time.Second}
		resp, err := c.Post(srv.URL+"/v1/chat/completions", "application/json", strings.NewReader(pastedImage))
		if err != nil {
			got <- reply{err: err}
			return
		}
		b, err := io.ReadAll(resp.Body)
		resp.Body.Close()
		got <- reply{resp.StatusCode, string(b), err}
	}()

	// shown as routing while it waits
	deadline := time.Now().Add(10 * time.Second)
	for {
		rs := routesOf(s, "probe/text")
		if len(rs) == 1 && !rs[0].Done {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("the held request never showed on the Routing page: %+v", rs)
		}
		time.Sleep(10 * time.Millisecond)
	}

	var r reply
	select {
	case r = <-got:
	case <-time.After(15 * time.Second):
		t.Fatal("the held request was never answered")
	}
	if r.err != nil || r.code != http.StatusGatewayTimeout || !strings.Contains(r.body, "without routing it to any provider") {
		t.Fatalf("held request answered %d %q %v", r.code, r.body, r.err)
	}
	rs := routesOf(s, "probe/text")
	if len(rs) != 1 || !rs[0].Done || rs[0].Status != http.StatusGatewayTimeout || !strings.Contains(rs[0].Error, "without routing") {
		t.Fatalf("the held request's route: %+v", rs)
	}
}

// One shown while it waited gives way to its own route once routed: the
// Routing page has the request once, and its reply is the provider's.
func TestUnroutedShownGivesWayToItsRoute(t *testing.T) {
	unroutedLimits(t, 50*time.Millisecond, time.Minute)
	s, u := slowEyed(t, 0)
	u.open()
	release := heldSight(t, s)
	done := make(chan int, 1)
	go func() {
		code, _ := postAs(t, s, "", pastedImage)
		done <- code
	}()
	deadline := time.Now().Add(10 * time.Second)
	for len(routesOf(s, "probe/text")) == 0 {
		if time.Now().After(deadline) {
			t.Fatal("the held request never showed on the Routing page")
		}
		time.Sleep(10 * time.Millisecond)
	}
	release("HELLO")
	select {
	case code := <-done:
		if code != 200 {
			t.Fatalf("routed after its wait: %d", code)
		}
	case <-time.After(15 * time.Second):
		t.Fatal("never answered once let go")
	}
	rs := routesOf(s, "probe/text")
	if len(rs) != 1 || rs[0].Provider != "probe" || rs[0].Status != 200 {
		t.Fatalf("routes for the request: %+v", rs)
	}
}

// A request answered before it was shown leaves nothing on the trace of
// its wait, and a timer that fires as its handler returns adds nothing.
func TestUnroutedQuickRequestLeavesNoTrace(t *testing.T) {
	unroutedLimits(t, time.Millisecond, time.Minute)
	s, _ := slowEyed(t, 0)
	for range 20 {
		code, _ := postAs(t, s, "", `{"model":"nowhere/none","messages":[{"role":"user","content":"hi"}]}`)
		if code < 400 {
			t.Fatalf("an unknown model answered %d", code)
		}
	}
	time.Sleep(50 * time.Millisecond)
	for _, r := range s.Trace(context.Background(), 0, 0).Routes {
		if r.Provider == "magpie" && !r.Done {
			t.Fatalf("a route left showing a request that was answered: %+v", r)
		}
	}
}
