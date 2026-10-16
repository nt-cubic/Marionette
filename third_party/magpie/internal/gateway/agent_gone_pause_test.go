package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// unreachable answers every request as a relay does that can't reach its
// own upstream (heng91 on #1894: a relay behind a proxy that was off).
type unreachable struct{ n atomic.Int32 }

func (u *unreachable) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	io.ReadAll(r.Body)
	u.n.Add(1)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusServiceUnavailable)
	io.WriteString(w, `{"error":{"type":"api_error","message":"upstream connect error or disconnect/reset before headers"}}`)
}

// An agent closed while magpie waits to ask a failing upstream again (#1894,
// heng91: the Claude Code or Codex window closed while its relay was down)
// ends the request there: the upstream is asked nothing more, the route is
// over with nothing in flight, and its last try says the agent left in the
// pause, so the Routing page doesn't end its story on a retry that never
// came. Both pauses: the last one left asked again, and a group's members
// asked again in turn (pauseAlive).
func TestAgentGoneInTheRetryPause(t *testing.T) {
	for _, group := range []bool{false, true} {
		name := "last one left"
		if group {
			name = "group replan"
		}
		t.Run(name, func(t *testing.T) {
			fresh(t)
			old := retryPause
			retryPause = 2 * time.Second // the pause the agent leaves in
			t.Cleanup(func() { retryPause = old })
			up := &unreachable{}
			srv := httptest.NewServer(up)
			t.Cleanup(srv.Close)
			model := "a/m"
			for _, id := range []string{"a", "b"} {
				if err := provider.Save(provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Models: []string{"m"}, Anthropic: srv.URL}); err != nil {
					t.Fatal(err)
				}
			}
			want := int32(1)
			if group {
				if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
					t.Fatal(err)
				}
				model, want = "group/g", 2
			}
			s := New()
			gw := httptest.NewServer(s.Handler())
			t.Cleanup(gw.Close)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			req, _ := http.NewRequestWithContext(ctx, "POST", gw.URL+"/v1/messages",
				strings.NewReader(`{"model":"`+model+`","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"hi"}]}`))
			req.Header.Set("Content-Type", "application/json")
			ended := make(chan struct{})
			go func() {
				defer close(ended)
				if res, err := http.DefaultClient.Do(req); err == nil {
					io.Copy(io.Discard, res.Body)
					res.Body.Close()
				}
			}()
			// every candidate has failed once: magpie is in its pause
			deadline := time.Now().Add(10 * time.Second)
			for up.n.Load() < want && time.Now().Before(deadline) {
				time.Sleep(5 * time.Millisecond)
			}
			if up.n.Load() != want {
				t.Fatalf("upstream asked %d times before the pause, want %d", up.n.Load(), want)
			}
			// and its answer has been read: the last try says how long the
			// pause is (a fixed sleep here cancelled the last try still in
			// flight on a loaded runner)
			if !inPause(s, int(want), 10*time.Second) {
				t.Fatal("the last try never started its pause")
			}
			cancel() // the agent's window is closed
			<-ended
			// the route ends at once, not when the pause would have
			var r Route
			for wait := time.Now().Add(time.Second); ; time.Sleep(5 * time.Millisecond) {
				s.trace.mu.Lock()
				r = *s.trace.routes[len(s.trace.routes)-1]
				r.Tries = append([]Try{}, r.Tries...)
				s.trace.mu.Unlock()
				if r.Done || time.Now().After(wait) {
					break
				}
			}
			// and nobody is asked again once the pause has gone by
			time.Sleep(3 * retryPause)
			if n := up.n.Load(); n != want {
				t.Errorf("upstream asked %d times after the agent left, want %d", n, want)
			}
			if !r.Done || r.Status != 499 {
				t.Fatalf("route done %v status %d, want done 499", r.Done, r.Status)
			}
			for _, tr := range r.Tries {
				if !tr.Done {
					t.Errorf("a try still in flight: %+v", tr)
				}
			}
			last := r.Tries[len(r.Tries)-1]
			if last.Again == 0 || !last.Gone {
				t.Errorf("last try %+v: want its pause and that the agent left in it", last)
			}
			if group && last.Replan == 0 {
				t.Errorf("last try %+v: want the group's replan", last)
			}
		})
	}
}

// inPause tells whether the last route's tries number n and the last of
// them waits to be asked again, polled for up to within.
func inPause(s *Server, n int, within time.Duration) bool {
	for deadline := time.Now().Add(within); time.Now().Before(deadline); time.Sleep(5 * time.Millisecond) {
		s.trace.mu.Lock()
		ok := false
		if k := len(s.trace.routes); k > 0 {
			tries := s.trace.routes[k-1].Tries
			ok = len(tries) == n && tries[n-1].Again > 0
		}
		s.trace.mu.Unlock()
		if ok {
			return true
		}
	}
	return false
}
