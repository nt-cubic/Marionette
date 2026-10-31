package gateway

import (
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// plugins#512 (cycocyco): SenseTime Raccoon's 429 for an account's token
// throughput used up, word for word. It has no Retry-After.
const raccoonWall = `{"error":{"code":"user_model_rate_limited","message":"您的模型请求频率超限，请稍后重试","type":"rate_limit_error"}}`

// The wall is a rate limit, rested as one, but no moment clears it: it is
// not asked again in place, by the same-one retries or a patience's
// rounds. A rate limit without its code still is.
func TestTokenWallIsNotRetriedInPlace(t *testing.T) {
	if got := failure(429, []byte(raccoonWall)); got != failRate {
		t.Fatalf("failure: %s, want %s", got, failRate)
	}
	if _, ok := passing(429, nil, []byte(raccoonWall), 0); ok {
		t.Error("the wall passes: asked again in place")
	}
	if busy(429, []byte(raccoonWall)) {
		t.Error("the wall is busy: asked again in a patience's rounds")
	}
	// the same words with another code, or the code in a message, are a
	// rate limit that passes, as before
	for _, body := range []string{
		`{"error":{"code":"rate_limited","message":"您的模型请求频率超限，请稍后重试","type":"rate_limit_error"}}`,
		`{"error":{"message":"Rate limit exceeded (user_model_rate_limited), please try again later"}}`,
		`{"error":{"code":429,"message":"Rate limit exceeded"}}`,
	} {
		if _, ok := passing(429, nil, []byte(body), 0); !ok {
			t.Errorf("%s: doesn't pass", body)
		}
		if !busy(429, []byte(body)) {
			t.Errorf("%s: not busy", body)
		}
	}
}

// The reporter's case: the lone account's wall reaches the agent after one
// ask, not eight in 38s; in a group the next member answers, and the
// walled one rests, so the next request isn't sent to it at all.
func TestTokenWallFailsFast(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{429, "", raccoonWall}}}
	scriptedOn(t, "a", provider.Chat, a)
	start := time.Now()
	code, body := postAs(t, New(), "", `{"model":"a/m","messages":[{"role":"user","content":"hi"}]}`)
	if code != 429 || a.n != 1 || !strings.Contains(body, "user_model_rate_limited") && !strings.Contains(body, "频率超限") {
		t.Fatalf("%d after %d tries: %s", code, a.n, body)
	}
	if d := time.Since(start); d > 5*time.Second {
		t.Fatalf("the agent had it after %v", d)
	}

	fresh(t)
	a = &scripted{replies: []reply{{429, "", raccoonWall}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	for i := range 2 {
		code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
		if code != 200 || !strings.Contains(body, "hello") || a.n != 1 || b.n != i+1 {
			t.Fatalf("request %d: %d %s (a %d, b %d)", i+1, code, body, a.n, b.n)
		}
	}
}
