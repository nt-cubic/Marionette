package gateway

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

// The bucket tolerates a burst of wrong remote keys, then turns the next one
// away until a token refills, on an injected clock so the test is not timed.
func TestAuthThrottleBurstThenRefills(t *testing.T) {
	resetAuthThrottle()
	t.Cleanup(resetAuthThrottle)
	cur := time.Unix(1_700_000_000, 0)
	authThrottle.Lock()
	authThrottle.now = func() time.Time { return cur }
	authThrottle.Unlock()

	for i := 0; i < authFailBurst; i++ {
		if over, _ := tooManyAuthFails(); over {
			t.Fatalf("turned away within the burst, at try %d", i+1)
		}
	}
	over, retry := tooManyAuthFails()
	if !over || retry <= 0 {
		t.Fatalf("one past the burst: over=%v retry=%v, want over with a wait", over, retry)
	}
	// Still clamped a moment later.
	cur = cur.Add(authFailRefill / 2)
	if over, _ := tooManyAuthFails(); !over {
		t.Fatal("let a wrong key through before a token refilled")
	}
	// A full refill period frees exactly one token.
	cur = cur.Add(authFailRefill)
	if over, _ := tooManyAuthFails(); over {
		t.Fatal("still clamped after a token refilled")
	}
	if over, _ := tooManyAuthFails(); !over {
		t.Fatal("let a second wrong key through on one refilled token")
	}
}

// A flood of wrong remote keys ends in 429s with Retry-After, but a valid
// key and a request from this computer are never held — the real caller
// gets in while the attacker is clamped.
func TestAuthThrottleSparesValidAndLocal(t *testing.T) {
	fresh(t) // resets the bucket
	t.Setenv("MAGPIE_ADDR", "")
	h := lanGuard(New().Handler())
	_, secrets := newCaller(t, "LAN client")
	key := secrets[0]

	// Hold the clock so no token refills mid-test: the flood must stay
	// clamped from the moment it fills the bucket.
	cur := time.Unix(1_700_000_000, 0)
	authThrottle.Lock()
	authThrottle.now = func() time.Time { return cur }
	authThrottle.Unlock()
	t.Cleanup(resetAuthThrottle)

	call := func(from string, hdr ...string) (int, http.Header) {
		r := httptest.NewRequest("GET", "/v1/magpie/quotas", nil)
		r.RemoteAddr = from
		for i := 0; i+1 < len(hdr); i += 2 {
			r.Header.Set(hdr[i], hdr[i+1])
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code, w.Header()
	}

	got401, got429 := 0, 0
	for i := 0; i < authFailBurst+10; i++ {
		c, hdr := call("192.168.1.9:5000", "Authorization", "Bearer wrong-key-value")
		switch c {
		case http.StatusUnauthorized:
			got401++
		case http.StatusTooManyRequests:
			got429++
			if hdr.Get("Retry-After") == "" {
				t.Fatal("a 429 with no Retry-After")
			}
		default:
			t.Fatalf("a wrong key got %d", c)
		}
	}
	if got401 < authFailBurst {
		t.Fatalf("clamped before the burst was spent: only %d of %d got 401", got401, authFailBurst)
	}
	if got429 == 0 {
		t.Fatal("a flood of wrong keys was never clamped to 429")
	}

	// The valid key is answered even though the bucket is empty.
	if c, _ := call("192.168.1.9:5000", "Authorization", "Bearer "+key); c != 200 {
		t.Fatalf("a valid key was held during the flood: got %d", c)
	}
	// A request from this computer is never counted or clamped.
	for i := 0; i < 5; i++ {
		if c, _ := call("127.0.0.1:5000"); c != 200 {
			t.Fatalf("a loopback request was clamped: got %d", c)
		}
	}
	// A wrong key is still clamped right after.
	if c, _ := call("192.168.1.9:5000", "Authorization", "Bearer wrong-key-value"); c != http.StatusTooManyRequests {
		t.Fatalf("the flood cleared when it should not have: got %d", c)
	}
}

// Before the cap is reached, a wrong key still gets the helpful 401, not a
// 429 (the burst does not swallow ordinary mistypes).
func TestAuthThrottleKeepsFirstRefusalHelpful(t *testing.T) {
	fresh(t)
	t.Setenv("MAGPIE_ADDR", "")
	h := lanGuard(New().Handler())
	newCaller(t, "LAN client")
	r := httptest.NewRequest("GET", "/v1/magpie/quotas", nil)
	r.RemoteAddr = "10.0.0.7:5000"
	r.Header.Set("Authorization", "Bearer mgp-typo-key-9z7q")
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != http.StatusUnauthorized || !strings.Contains(w.Body.String(), "ending in 9z7q") {
		t.Fatalf("first wrong key not a helpful 401: %d %s", w.Code, w.Body.String())
	}
}
