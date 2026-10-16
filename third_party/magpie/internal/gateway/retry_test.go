package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

func init() { retryPause = time.Millisecond }

// scripted answers each request with the next of its replies, the last
// one over and over.
type scripted struct {
	replies []reply
	n       int
	hang    chan struct{} // when set, the first request waits on it
	hanging chan struct{} // when set, closed as the first request begins to wait on hang
	limited http.Header   // headers each 429 goes with
}

type reply struct {
	code  int
	ctype string
	body  string
}

func (s *scripted) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	io.ReadAll(r.Body)
	i := min(s.n, len(s.replies)-1)
	s.n++
	if s.hang != nil && i == 0 {
		if s.hanging != nil {
			close(s.hanging)
		}
		<-s.hang
	}
	x := s.replies[i]
	if x.ctype == "" {
		x.ctype = "application/json"
	}
	w.Header().Set("Content-Type", x.ctype)
	if x.code == http.StatusTooManyRequests {
		for k, vs := range s.limited {
			w.Header()[k] = vs
		}
	}
	if x.code != 0 {
		w.WriteHeader(x.code)
	}
	io.WriteString(w, x.body)
}

const chatOK = `{"id":"ok","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]}`

func scriptedOn(t *testing.T, id string, proto provider.Protocol, s *scripted) {
	t.Helper()
	up := httptest.NewServer(s)
	t.Cleanup(up.Close)
	p := provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Models: []string{"m"}}
	switch proto {
	case provider.Anthropic:
		p.Anthropic = up.URL
	default:
		p.Chat = up.URL + "/v1"
	}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
}

// A provider that can't serve the model — turned away, not the request at
// fault — hands the request to the next member rather than to the agent.
func TestGroupFailsOverAnUnservedModel(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{400, "", `{"error":{"message":"model \"gpt-6-sol\" is not accessible via the /chat/completions endpoint","code":"unsupported_api_for_model"}}`}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 1 || b.n != 1 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	for _, x := range []struct {
		status int
		body   string
		want   bool
	}{
		{401, `{"error":{"message":"invalid api key"}}`, true},
		{404, `{"error":{"message":"not found"}}`, true},
		{400, `{"error":{"message":"The model gpt-x does not exist"}}`, true},
		{400, `{"error":{"message":"messages: field required"}}`, false},
		{400, `{"error":{"message":"This model's maximum context length is 128000 tokens"}}`, false},
		{400, `{"error":{"message":"Illegal API invocation from an unapproved channel"}}`, true},
	} {
		if got := retryable(x.status, []byte(x.body)); got != x.want {
			t.Errorf("retryable(%d, %s) = %v", x.status, x.body, got)
		}
	}
}

// An error a stream begins with, before any of its content, is a failure
// the next one answers, as an error status is; one that comes after
// content has begun reaches the agent as it came.
func TestStreamErrorBeforeContentFailsOver(t *testing.T) {
	fresh(t)
	overloaded := sse(`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"m1","role":"assistant","content":[],"usage":{"input_tokens":3}}}`,
		`event: ping`+"\n"+`data: {"type":"ping"}`,
		`event: error`+"\n"+`data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`)
	good := sse(`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"m2","role":"assistant","content":[],"usage":{"input_tokens":3}}}`,
		`event: content_block_start`+"\n"+`data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}`,
		`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"from b"}}`,
		`event: message_stop`+"\n"+`data: {"type":"message_stop"}`)
	a := &scripted{replies: []reply{{200, "text/event-stream", overloaded}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", good}}}
	scriptedOn(t, "a", provider.Anthropic, a)
	scriptedOn(t, "b", provider.Anthropic, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	s := New()
	send := func() (int, string) {
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/messages",
			strings.NewReader(`{"model":"group/g","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"hi"}]}`)))
		return rec.Code, rec.Body.String()
	}
	code, body := send()
	if code != 200 || !strings.Contains(body, "from b") || strings.Contains(body, "Overloaded") || strings.Contains(body, `"m1"`) {
		t.Fatalf("%d %s %+v", code, body, s.trace.routes[len(s.trace.routes)-1])
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 2 || r.Tries[0].Status != 529 || r.Tries[0].Rest == nil {
		t.Fatalf("tries: %+v", r.Tries)
	}

	// after content, the error is the agent's to see
	fresh(t)
	late := sse(`event: message_start`+"\n"+`data: {"type":"message_start","message":{"id":"m1","role":"assistant","content":[]}}`,
		`event: content_block_delta`+"\n"+`data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half"}}`,
		`event: error`+"\n"+`data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`)
	a = &scripted{replies: []reply{{200, "text/event-stream", late}}}
	b = &scripted{replies: []reply{{200, "text/event-stream", good}}}
	scriptedOn(t, "a", provider.Anthropic, a)
	scriptedOn(t, "b", provider.Anthropic, b)
	provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered})
	s = New()
	if code, body := send(); code != 200 || !strings.Contains(body, "half") || !strings.Contains(body, "Overloaded") || b.n != 0 {
		t.Fatalf("late: %d %s (b %d)", code, body, b.n)
	}

	for _, x := range []struct {
		ev   string
		kind int
	}{
		{`: keep-alive`, eventLead},
		{`data: {"id":"c","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}`, eventLead},
		{`data: {"id":"c","choices":[{"index":0,"delta":{"content":"hi"}}]}`, eventContent},
		{`data: {"error":{"message":"upstream failed"}}`, eventError},
		{`event: response.created` + "\n" + `data: {"type":"response.created","response":{"id":"r"}}`, eventLead},
		{`event: codex.rate_limits` + "\n" + `data: {"type":"codex.rate_limits","plan_type":"plus","rate_limits":{"allowed":true}}`, eventLead},
		{`event: response.failed` + "\n" + `data: {"type":"response.failed","response":{"error":{"code":"server_error","message":"boom"}}}`, eventError},
		{`event: response.output_text.delta` + "\n" + `data: {"type":"response.output_text.delta","delta":"hi"}`, eventContent},
		{`data: [DONE]`, eventContent},
	} {
		if k, _, _ := streamEvent([]byte(x.ev)); k != x.kind {
			t.Errorf("streamEvent(%s) = %d, want %d", x.ev, k, x.kind)
		}
	}
}

// The last one left — or the only one — is tried again after a failure
// that may pass, a moment later; one that won't is the agent's at once.
func TestLastOneLeftIsTriedAgain(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{503, "", `{"error":{"message":"upstream connect error"}}`}, {502, "", `{"error":{"message":"bad gateway"}}`}, {200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	s := New()
	code, body := postAs(t, s, "", `{"model":"a/m","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 3 {
		t.Fatalf("%d %s (tried %d times)", code, body, a.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 3 || r.Tries[0].Again == 0 || r.Tries[1].Again == 0 || r.Tries[2].Status != 200 {
		t.Fatalf("tries: %+v", r.Tries)
	}

	fresh(t)
	a = &scripted{replies: []reply{{503, "", `{"error":{"message":"down"}}`}}}
	scriptedOn(t, "a", provider.Chat, a)
	if code, _ := postAs(t, New(), "", `{"model":"a/m","messages":[{"role":"user","content":"hi"}]}`); code != 503 || a.n != 1+lastRetries {
		t.Fatalf("gave up with %d after %d tries", code, a.n)
	}

	fresh(t)
	a = &scripted{replies: []reply{{400, "", `{"error":{"message":"messages: field required"}}`}}}
	scriptedOn(t, "a", provider.Chat, a)
	if code, _ := postAs(t, New(), "", `{"model":"a/m","messages":[{"role":"user","content":"hi"}]}`); code != 400 || a.n != 1 {
		t.Fatalf("a request at fault: %d after %d tries", code, a.n)
	}
}

// A rate limit on the last one left is waited out, a moment, whether the
// vendor says how long or not (#503: a relay's 429 with no Retry-After went
// straight to the agent); one that won't clear in seconds — a plan used up,
// no money left, a Retry-After an hour away — is the agent's at once.
func TestRateLimitIsWaitedOut(t *testing.T) {
	limited := `{"error":{"message":"Rate limit exceeded, please try again later"}}`
	for _, x := range []struct {
		name    string
		said    string
		header  http.Header
		times   int // how many times it is limited first
		code, n int
	}{
		{"retry-after", limited, http.Header{"Retry-After": {"1"}}, 1, 200, 2},
		{"no retry-after", limited, nil, 1, 200, 2},
		{"relay's own words", `{"error":{"message":"当前分组上游负载已饱和，请稍后再试"}}`, nil, 2, 200, 3},
		{"thrice", limited, nil, 3, 200, 4},
		{"too long", limited, nil, 9, 429, 1 + rateRetries},
		{"usage limit", `{"error":{"message":"You've hit your usage limit. Your limit resets in 5 hours."}}`, nil, 9, 429, 1},
		{"no balance", `{"error":{"message":"余额不足"}}`, nil, 9, 429, 1},
		{"an hour away", limited, http.Header{"Retry-After": {"3600"}}, 9, 429, 1},
	} {
		fresh(t)
		a := &scripted{limited: x.header}
		for range x.times {
			a.replies = append(a.replies, reply{429, "", x.said})
		}
		a.replies = append(a.replies, reply{200, "", chatOK})
		scriptedOn(t, "a", provider.Chat, a)
		start := time.Now()
		code, body := postAs(t, New(), "", `{"model":"a/m","messages":[{"role":"user","content":"hi"}]}`)
		if code != x.code || a.n != x.n {
			t.Errorf("%s: %d after %d tries: %s", x.name, code, a.n, body)
		}
		if x.n == 1 && time.Since(start) > time.Second {
			t.Errorf("%s: passed on after %v", x.name, time.Since(start))
		}
	}
}

// An agent that goes away mid-request isn't a provider failing: nobody
// rests for it and nobody else is asked.
func TestCanceledRequestRestsNobody(t *testing.T) {
	fresh(t)
	a := &scripted{replies: []reply{{200, "", chatOK}}, hang: make(chan struct{}), hanging: make(chan struct{})}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered})
	s := New()
	ctx, cancel := context.WithCancel(context.Background())
	served := make(chan struct{})
	// the agent goes away while a is asked (not a moment after the request
	// began: on a busy machine a may not be asked yet, #2175), and a never
	// answers until the gateway is done with the request
	go func() {
		select {
		case <-a.hanging:
			cancel()
		case <-served:
		}
		select {
		case <-served:
		case <-time.After(10 * time.Second):
		}
		close(a.hang)
	}()
	rec := httptest.NewRecorder()
	s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/chat/completions",
		strings.NewReader(`{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)).WithContext(ctx))
	close(served)
	r := s.trace.routes[len(s.trace.routes)-1]
	if b.n != 0 || len(r.Tries) != 1 || r.Tries[0].Fail != failCanceled || r.Tries[0].Rest != nil || s.resting("a") {
		t.Fatalf("b tried %d, tries %+v", b.n, r.Tries)
	}
}
