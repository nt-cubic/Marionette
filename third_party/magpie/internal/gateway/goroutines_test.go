package gateway

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/settings"
)

// parkedForTheDump waits on ch, as a goroutine stuck in magpie would: the
// dump must name it and what it waits on.
func parkedForTheDump(ch <-chan struct{}, parked chan<- struct{}) {
	close(parked)
	<-ch
}

// The goroutine dump goes to the same callers as the heap profile (a
// program on this computer over loopback, nothing a browser sends, no
// other Host) and is refused to the rest. What it answers is every
// goroutine's stack, a goroutine parked in magpie's code among them,
// named with what it waits on, and which process it is of (#3362: an app
// stuck for days could only be read by killing it).
func TestGoroutineDumpOnlyToThisComputer(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	_, secrets := newCaller(t, "Page")
	s := settings.Load()
	s.CORSOrigins = []string{"https://listed.example"}
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	key := secrets[0]
	srv := New()
	call := func(guarded bool, remote, host string, hdr ...string) *httptest.ResponseRecorder {
		t.Helper()
		r := httptest.NewRequest("GET", GoroutinesPath, nil)
		r.RemoteAddr = remote
		r.Host = host
		for i := 0; i+1 < len(hdr); i += 2 {
			r.Header.Set(hdr[i], hdr[i+1])
		}
		w := httptest.NewRecorder()
		h := srv.Handler()
		if guarded {
			h = lanGuard(h)
		}
		h.ServeHTTP(w, r)
		return w
	}
	for _, c := range profileCallersRefused(key) {
		for _, guarded := range []bool{true, false} {
			w := call(guarded, c.remote, c.host, c.hdr...)
			// lanGuard asks another machine without a key for one (401)
			if w.Code != http.StatusForbidden && w.Code != http.StatusUnauthorized || w.Header().Get(GoroutineStatsHeader) != "" || bytes.HasPrefix(w.Body.Bytes(), []byte("goroutine ")) {
				t.Errorf("%s (lanGuard %v): %d %q", c.name, guarded, w.Code, w.Body.String())
			}
		}
	}

	release, parked := make(chan struct{}), make(chan struct{})
	defer close(release)
	go parkedForTheDump(release, parked)
	<-parked
	for _, host := range []string{"127.0.0.1:3425", "localhost:3425", "[::1]:3425"} {
		w := call(true, "127.0.0.1:50123", host, "User-Agent", "Go-http-client/1.1")
		if w.Code != http.StatusOK {
			t.Fatalf("magpie goroutines at %s: %d %q", host, w.Code, w.Body.String())
		}
		body := w.Body.String()
		if !strings.HasPrefix(body, "goroutine ") {
			t.Fatalf("at %s: not a goroutine dump: %.200q", host, body)
		}
		// the parked one, with its state and its frame in this file
		i := strings.Index(body, "gateway.parkedForTheDump(")
		if i < 0 {
			t.Fatalf("at %s: the parked goroutine isn't in the dump", host)
		}
		head := body[strings.LastIndex(body[:i], "goroutine "):i]
		if !strings.Contains(head, "[chan receive") {
			t.Errorf("at %s: the parked goroutine's state isn't its wait: %q", host, head)
		}
		if !strings.Contains(body[i:], "goroutines_test.go:") {
			t.Errorf("at %s: no source line for the parked goroutine", host)
		}
		var st GoroutineStats
		if err := json.Unmarshal([]byte(w.Header().Get(GoroutineStatsHeader)), &st); err != nil {
			t.Fatalf("at %s: stats %q: %v", host, w.Header().Get(GoroutineStatsHeader), err)
		}
		if st.PID != os.Getpid() || st.Goroutines < 2 || st.Uptime == "" {
			t.Errorf("at %s: stats %+v", host, st)
		}
		if !strings.Contains(w.Header().Get("Cache-Control"), "no-store") || !strings.HasPrefix(w.Header().Get("Content-Type"), "text/plain") {
			t.Errorf("at %s: headers %v", host, w.Header())
		}
	}
}
