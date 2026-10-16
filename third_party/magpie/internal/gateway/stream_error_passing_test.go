package gateway

import (
	"net/http"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// piRetryable is Pi's isRetryableError (agent-session.ts, 1.2.0), as
// zminds quoted it on #2372: the text alone decides, a deny-list first.
var (
	piRetry = regexp.MustCompile(`(?i)overloaded|rate.?limit|too many requests|429|500|502|503|504|520|524|service.?unavailable|server.?error|internal.?error|timed? out|timeout|terminated|connection.?error|connection.?refused|ended without|socket hang up|fetch failed|ENOTFOUND|EAI_AGAIN|websocket closed|retry delay|you can retry your request|try your request again|please retry your request|ResourceExhausted`)
	piDeny  = regexp.MustCompile(`(?i)insufficient_quota|quota exceeded|available balance|out of budget|billing|GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|subscription_sharing_usage_limit_exceeded`)
)

func piRetries(msg string) bool { return !piDeny.MatchString(msg) && piRetry.MatchString(msg) }

// An error that reaches the agent in a stream already begun can't say its
// status, and the words of one a moment may clear are said for it: Pi
// judges an error by its text alone, and ended the turn on each of these,
// which Claude Code and Codex asked again (#2372, the texts zminds' Pi
// sessions recorded). A request's own fault is left as it was said.
func TestStreamErrorSaysItPasses(t *testing.T) {
	for _, c := range []struct {
		status int
		msg    string
	}{
		{503, "Upstream service temporarily unavailable"},
		{502, "Upstream response stream was interrupted"},
		{504, "A: no first token in 30s"},
		{502, "Upstream request failed"},
		{502, "Upstream HTTP/2 stream failed"},
		{521, "521 status code (no body)"},
		{503, "The service is temporarily unavailable. Please retry later."},
		{429, "Too many pending requests, please retry later"},
		{408, "the request took too long"},
	} {
		if got := saysPassing(c.status, c.msg); !piRetries(got) || !strings.HasPrefix(got, c.msg) {
			t.Errorf("%d %q is told %q", c.status, c.msg, got)
		}
	}
	for _, msg := range []string{"Upstream rate limit exceeded, please retry later", "503: Service temporarily unavailable", "connection error"} {
		if got := saysPassing(503, msg); got != msg {
			t.Errorf("%q, already said, is told %q", msg, got)
		}
	}
	for _, c := range []struct {
		status int
		msg    string
	}{
		{400, "Invalid request parameters"},
		{401, "invalid api key"},
		{404, "model not found"},
	} {
		if got := saysPassing(c.status, c.msg); got != c.msg {
			t.Errorf("%d %q is told %q", c.status, c.msg, got)
		}
	}
}

// Through a group: a member let go for no first token, the stream's
// headers sent to keep the agent alive meanwhile, and the last member
// failing with a relay's words — the agent's stream error says what Pi
// asks again for, in each protocol it reads.
func TestGroupLastFailureInStreamSaysItPasses(t *testing.T) {
	old := retryPause
	retryPause = time.Millisecond
	t.Cleanup(func() { retryPause = old })
	k := keepHeldAfter
	keepHeldAfter = 30 * time.Millisecond
	t.Cleanup(func() { keepHeldAfter = k })
	fresh(t)
	keepFast(t, keepaliveGap, 20*time.Millisecond, keepaliveLongest)
	a := &slowStart{wait: 10 * time.Second}
	b := &scripted{replies: []reply{{503, "application/json", `{"error":{"message":"Upstream service temporarily unavailable","type":"upstream_error"}}`}}}
	slowOn(t, "a", a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, FirstToken: 1}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != http.StatusOK || !strings.Contains(body, ": keepalive") {
		t.Fatalf("the agent wasn't sent the stream's headers first: %d %q", code, body)
	}
	i := strings.Index(body, "Upstream service temporarily unavailable")
	if i < 0 {
		t.Fatalf("no error in the stream: %q", body)
	}
	said := body[i:]
	if j := strings.Index(said, `"`); j >= 0 {
		said = said[:j]
	}
	if !piRetries(said) {
		t.Fatalf("the stream's error %q is one Pi doesn't ask again for: %q", said, body)
	}
}
