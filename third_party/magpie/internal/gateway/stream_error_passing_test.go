package gateway

import (
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/tidwall/gjson"
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

// A vendor byte cap that lands after the stream's headers have already
// gone out (a keepalive while the vendor is still quiet) reaches the
// agent as an error event. Pi compacts when that text says
// request_too_large (overflow.ts), the same word a 413 JSON body carries
// before the stream starts. "Request Entity Too Large" does not, so the
// turn stops (@wilsonxuuuu, once the stream is already open).
func TestStreamedByteCapSaysRequestTooLargeToPi(t *testing.T) {
	quietFast(t)
	fresh(t)
	const nginx = "<html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n<body>\r\n<center><h1>413 Request Entity Too Large</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n"
	piOverflow := regexp.MustCompile(`(?i)request_too_large`)
	hang := make(chan struct{})
	scriptedOn(t, "a", provider.Chat, &scripted{hang: hang, replies: []reply{{413, "text/html", nginx}}})
	heard := make(chan struct{})
	go func() {
		select {
		case <-heard:
		case <-time.After(5 * time.Second):
		}
		close(hang)
	}()
	code, body, _ := askChat(t, New(), "a/m", heard)
	if !strings.Contains(body, ": keepalive") {
		t.Fatalf("the stream's headers never went out ahead of the 413: %d %q", code, body)
	}
	// Pi reads the error's message, not its type: the word is in the text
	var msg, typ string
	for _, line := range strings.Split(body, "\n") {
		if d, ok := strings.CutPrefix(line, "data: "); ok && gjson.Get(d, "error").Exists() {
			msg, typ = gjson.Get(d, "error.message").String(), gjson.Get(d, "error.type").String()
		}
	}
	if !piOverflow.MatchString(msg) || typ != "request_too_large" || strings.Contains(body, "context_length_exceeded") {
		t.Fatalf("byte cap in a stream already begun: %d %s", code, body)
	}
}

// Every shape a stream already begun says a 413 in (#2533): the text names
// request_too_large for Chat, Anthropic and Responses, and Chat's and
// Anthropic's type is it; Gemini keeps its own status shape; a 413 that
// says the context is too long stays context_length_exceeded.
func TestStreamErrorByteCapInEveryShape(t *testing.T) {
	const capped = "A: 413 Request Entity Too Large"
	for _, c := range []struct {
		proto      provider.Protocol
		msg        string
		text, kind string // what the event's message and type say
	}{
		{provider.Chat, capped, "request_too_large", `"type":"request_too_large"`},
		{provider.Anthropic, capped, "request_too_large", `"type":"request_too_large"`},
		{provider.Responses, capped, "request_too_large", `"code":"server_error"`},
		{provider.Gemini, capped, "", `"status"`},
		{provider.Chat, "A: 413 prompt is too long: 300000 tokens > 200000 maximum", "", `"code":"context_length_exceeded"`},
	} {
		rec := httptest.NewRecorder()
		streamError(rec, c.proto, http.StatusRequestEntityTooLarge, c.msg)
		body := rec.Body.String()
		if !strings.Contains(body, c.kind) {
			t.Errorf("%s %q: want %s in %s", c.proto, c.msg, c.kind, body)
		}
		if said := strings.Contains(body, "(request_too_large)"); said != (c.text != "") {
			t.Errorf("%s %q: the text says request_too_large: %v, want %v: %s", c.proto, c.msg, said, c.text != "", body)
		}
	}
}
