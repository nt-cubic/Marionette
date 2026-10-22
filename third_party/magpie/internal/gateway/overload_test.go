package gateway

import (
	"bufio"
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// What the ChatGPT backend sent Aliceonly's Codex on plus-B (#1418): a 200
// and the frames before a reply, nothing said, and 36s on the response
// failed with server_is_overloaded.
var overloadedLead = []string{
	`event: response.created` + "\n" + `data: {"type":"response.created","response":{"id":"r0","status":"in_progress"}}`,
	`event: response.in_progress` + "\n" + `data: {"type":"response.in_progress","response":{"id":"r0","status":"in_progress"}}`,
	`event: codex.rate_limits` + "\n" + `data: {"type":"codex.rate_limits","plan_type":"plus","rate_limits":{"allowed":true}}`,
}

const overloadedFailed = `event: response.failed` + "\n" + `data: {"type":"response.failed","response":{"id":"r0","status":"failed","error":{"code":"server_is_overloaded","message":"Our servers are currently overloaded. Please try again later."}}}`

// A stream with nothing but its frames, held past 15s while its agent is
// kept alive with comments, stays held: its server_is_overloaded after
// that is a failure the group's next member answers, not the agent's
// error (#1418). Before, the frame that came after 15s let the stream
// through, and the failure 21s later reached Codex as the turn's end with
// sub2api, next in the group, never asked. A Gemini stream, whose agent
// takes no comment, is let through at 15s as before.
func TestLeadOnlyStreamHeldWhileKeptAlive(t *testing.T) {
	held := func(proto provider.Protocol) (*holdWriter, *httptest.ResponseRecorder) {
		rec := httptest.NewRecorder()
		h := newHoldWriter(rec, true)
		h.alive, h.streams = &keptAlive{proto: proto}, true
		h.Header().Set("Content-Type", "text/event-stream")
		h.WriteHeader(200)
		io.WriteString(h, sse(overloadedLead[:2]...))
		h.since = time.Now().Add(-(holdLongest + 5*time.Second))
		io.WriteString(h, sse(overloadedLead[2]))
		io.WriteString(h, ": ping\n\n")
		return h, rec
	}
	h, rec := held(provider.Responses)
	if h.passing || strings.Contains(rec.Body.String(), "response.created") {
		t.Fatalf("let through after %s with nothing said: %q", holdLongest, rec.Body.String())
	}
	io.WriteString(h, sse(overloadedFailed))
	if !h.failed() || h.code() != 529 || !strings.Contains(h.failMsg, "server_is_overloaded") || strings.Contains(rec.Body.String(), "server_is_overloaded") {
		t.Fatalf("failed %v (%d %q); sent %q", h.failed(), h.code(), h.failMsg, rec.Body.String())
	}

	// held so long, it is let through as before
	h, rec = held(provider.Responses)
	h.since = time.Now().Add(-(holdLead + time.Second))
	io.WriteString(h, ": ping\n\n")
	if !h.passing || !strings.Contains(rec.Body.String(), "response.created") {
		t.Fatalf("held past %s: %q", holdLead, rec.Body.String())
	}

	h, rec = held(provider.Gemini)
	if !h.passing || !strings.Contains(rec.Body.String(), "response.created") {
		t.Fatalf("a stream whose agent takes no comment held past %s", holdLongest)
	}
}

// Every member of a group overloaded at once (#1418: plus-B, then
// sub2api, each server_is_overloaded) isn't the agent's error while the
// group's patience lasts: after a pause each is asked again, and the one
// back by then answers. Before, the last one's 529 ended Codex's turn.
// Patience off, the agent gets it as before; a Retry-After past the
// patience isn't waited for.
func TestGroupWaitsOutEveryMemberOverloaded(t *testing.T) {
	overloaded := reply{529, "", `{"error":{"message":"Our servers are currently overloaded. Please try again later.","code":"server_is_overloaded"}}`}
	group := func(patience int) (*scripted, *scripted) {
		fresh(t)
		a := &scripted{replies: []reply{overloaded, {200, "", chatOK}}}
		b := &scripted{replies: []reply{overloaded}}
		scriptedOn(t, "a", provider.Chat, a)
		scriptedOn(t, "b", provider.Chat, b)
		if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Patience: patience}); err != nil {
			t.Fatal(err)
		}
		return a, b
	}
	ask := `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`

	a, b := group(0)
	s := New()
	code, body := postAs(t, s, "", ask)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 2 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	var replanned *Try
	for i := range r.Tries {
		if r.Tries[i].Replan > 0 {
			replanned = &r.Tries[i]
		}
	}
	if replanned == nil || replanned.Replan != 2 || replanned.Again == 0 || replanned.Patience != provider.PatienceDefault {
		t.Fatalf("the trace doesn't say the members were asked again: %+v", r.Tries)
	}
	if last := r.Tries[len(r.Tries)-1]; last.Status != 200 {
		t.Fatalf("tries: %+v", r.Tries)
	}

	a, b = group(provider.PatienceOff)
	if code, _ := postAs(t, New(), "", ask); code != 529 || a.n != 1 {
		t.Fatalf("patience off: %d (a %d, b %d)", code, a.n, b.n)
	}

	a, b = group(5)
	limited := reply{429, "", `{"error":{"message":"Rate limit reached, please try again shortly"}}`}
	a.limited, b.limited = http.Header{"Retry-After": {"9"}}, http.Header{"Retry-After": {"9"}}
	a.replies[0], b.replies[0] = limited, limited
	began := time.Now()
	if code, _ := postAs(t, New(), "", ask); code != 429 || a.n != 1 || time.Since(began) > 3*time.Second {
		t.Fatalf("a Retry-After past the patience: %d after %s (a %d, b %d)", code, time.Since(began), a.n, b.n)
	}
}

// A relay's 500 that says its model's load has reached its limit
// (real-jacket on #1418: two keys of one relay in an order group, Codex,
// Patience at 4 minutes) passes like any overload. Its "上限" made it a
// used-up quota: no member was asked again, and each rested a quarter of
// an hour by "quota", renewed on every try.
func TestGroupWaitsOutRelayLoadLimit(t *testing.T) {
	fresh(t)
	loaded := reply{500, "", `当前模型 gpt-5.5 负载已经达到上限，请稍后重试 (request id: 20261011161901234567890abcdefgh)`}
	a := &scripted{replies: []reply{loaded, {200, "", chatOK}}}
	b := &scripted{replies: []reply{loaded}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Patience: 240}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 || !strings.Contains(body, "hello") || a.n != 2 {
		t.Fatalf("%d %s (a %d, b %d)", code, body, a.n, b.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	replanned := false
	for _, try := range r.Tries {
		if try.Fail == failQuota || try.Rest != nil && try.Rest.By == "quota" {
			t.Errorf("a load limit read as a used-up quota: %+v", try)
		}
		replanned = replanned || try.Replan > 0
	}
	if !replanned {
		t.Fatalf("the members weren't asked again: %+v", r.Tries)
	}
}

// What Codex was sent (#1418): plus-B's 200 and frames, then
// server_is_overloaded, and sub2api the same. With patience, the agent's
// stream is the reply of the one asked again, and nothing of the failures.
func TestCodexStreamWaitsOutOverloadedGroup(t *testing.T) {
	fresh(t)
	failed := reply{200, "text/event-stream", sse(append(slices.Clone(overloadedLead), overloadedFailed)...)}
	done := reply{200, "text/event-stream", sse(
		`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","delta":"hello"}`,
		`event: response.completed`+"\n"+`data: {"type":"response.completed","response":{"id":"r1","status":"completed","output":[]}}`)}
	a := &scripted{replies: []reply{failed, done}}
	b := &scripted{replies: []reply{failed}}
	responsesOn(t, "a", a)
	responsesOn(t, "b", b)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	code, body := sendTo(New(), "/v1/responses", `{"model":"group/g","stream":true,"input":"hi"}`)
	if code != 200 || !strings.Contains(body, "hello") || strings.Contains(body, "server_is_overloaded") || a.n != 2 {
		t.Fatalf("%d %q (a %d, b %d)", code, body, a.n, b.n)
	}
}

// slowFrames is a Responses upstream that sends the frames before a reply
// at once and, after wait, its end: then (the request's n-th, from 0).
type slowFrames struct {
	wait time.Duration
	then func(n int) string
	mu   sync.Mutex
	n    int
}

func (s *slowFrames) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	io.ReadAll(r.Body)
	if !strings.HasSuffix(r.URL.Path, "/responses") {
		http.NotFound(w, r)
		return
	}
	s.mu.Lock()
	n := s.n
	s.n++
	s.mu.Unlock()
	w.Header().Set("Content-Type", "text/event-stream")
	w.WriteHeader(200)
	io.WriteString(w, sse(overloadedLead...))
	w.(http.Flusher).Flush()
	select {
	case <-time.After(s.wait):
	case <-r.Context().Done():
		return
	}
	io.WriteString(w, s.then(n))
}

// Codex counts only events toward its stream idle timeout, not the
// comments it is kept alive with (fadenoob on Discord: "stream
// disconnected before completion: idle timeout waiting for SSE", Codex
// reconnecting turn after turn). Each held try waited up to its own 4
// minutes, so a second one held after a first that failed late kept Codex
// past its 300s with no event. Now, once Codex has gone agentQuietMost
// with none, the try held goes to it as it is, and the next one's reply
// follows; a group's pause that would run past it isn't waited.
//
// Room on both sides (#1800): a fails 1.3s before Codex's quiet limit
// here, so a busy machine's lag doesn't carry a's failure past it to go
// as it is; the bounds sit over a second from both what is expected and
// what the old waits would take.
func TestCodexHeardWithinItsIdleTimeout(t *testing.T) {
	k, w, q := keepHeldAfter, watchEvery, agentQuietMost
	keepHeldAfter, watchEvery, agentQuietMost = 50*time.Millisecond, 10*time.Millisecond, 2*time.Second
	t.Cleanup(func() { keepHeldAfter, watchEvery, agentQuietMost = k, w, q })
	fresh(t)
	const wait = 4 * time.Second
	a := &slowFrames{wait: 700 * time.Millisecond, then: func(int) string { return sse(overloadedFailed) }}
	b := &slowFrames{wait: wait, then: func(int) string {
		return sse(`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","delta":"hello"}`,
			`event: response.completed`+"\n"+`data: {"type":"response.completed","response":{"id":"r1","status":"completed","output":[]}}`)
	}}
	for id, up := range map[string]http.Handler{"a": a, "b": b} {
		srv := httptest.NewServer(up)
		t.Cleanup(srv.Close)
		if err := provider.Save(provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Models: []string{"m"}, Responses: srv.URL + "/v1"}); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	gw := httptest.NewServer(New().Handler())
	t.Cleanup(gw.Close)
	start := time.Now()
	res, err := http.Post(gw.URL+"/v1/responses", "application/json", strings.NewReader(codexAsk))
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var first time.Duration
	var got strings.Builder
	rd := bufio.NewReader(res.Body)
	for {
		ln, err := rd.ReadString('\n')
		got.WriteString(ln)
		if first == 0 && strings.HasPrefix(ln, "data:") {
			first = time.Since(start)
		}
		if err != nil {
			break
		}
	}
	// a failed at 0.7s and b, held, would have gone at 4.7s: b's frames
	// go at 2s, Codex's idle timeout here, and its reply after them
	if first == 0 || first > 3500*time.Millisecond {
		t.Fatalf("Codex's first event came after %s:\n%s", first, got.String())
	}
	if !strings.Contains(got.String(), "hello") || strings.Contains(got.String(), "server_is_overloaded") {
		t.Fatalf("the stream:\n%s", got.String())
	}

	// a group's pause past it: Codex has the failure, not the wait
	fresh(t)
	a.n, b.n = 0, 0
	a.wait, b.wait = 600*time.Millisecond, 600*time.Millisecond
	b.then = a.then
	k2 := retryPause
	retryPause = 10 * time.Second
	t.Cleanup(func() { retryPause = k2 })
	for id, up := range map[string]http.Handler{"a": a, "b": b} {
		srv := httptest.NewServer(up)
		t.Cleanup(srv.Close)
		if err := provider.Save(provider.Provider{ID: id, Name: strings.ToUpper(id), Key: "k", Models: []string{"m"}, Responses: srv.URL + "/v1"}); err != nil {
			t.Fatal(err)
		}
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	gw2 := httptest.NewServer(New().Handler())
	t.Cleanup(gw2.Close)
	start = time.Now()
	res2, err := http.Post(gw2.URL+"/v1/responses", "application/json", strings.NewReader(codexAsk))
	if err != nil {
		t.Fatal(err)
	}
	body, _ := io.ReadAll(res2.Body)
	res2.Body.Close()
	// a's failure at 0.6s and b's at 1.2s; the pause waited would make it
	// 11.2s
	if took := time.Since(start); took > 6*time.Second || !strings.Contains(string(body), "server_is_overloaded") {
		t.Fatalf("after %s (a %d, b %d):\n%s", took, a.n, b.n, body)
	}
}

// Aliceonly's trace on #1418: sub2api answered 502 "Upstream HTTP/2
// stream failed", Codex account A a 200 that only framed a reply and then
// failed server_is_overloaded, and five candidates after it were never
// asked. An event streamEvent didn't know was taken for the reply's
// content and let the stream through, so its failure came after the
// reply had begun. Which event Codex's backend sent isn't in the trace;
// these stand in for any it may add. Now only text, reasoning or a call
// lets a stream through, and the third member answers.
func TestFramesThenOverloadedGoesToTheNext(t *testing.T) {
	fresh(t)
	unknown := []string{
		`event: keepalive` + "\n" + `data: {"type":"keepalive"}`,
		`event: response.rate_limits.updated` + "\n" + `data: {"type":"response.rate_limits.updated","rate_limits":{"allowed":true}}`,
		`event: response.output_item.added` + "\n" + `data: {"type":"response.output_item.added","item":{"type":"compaction_summary","id":"cs_1"}}`,
	}
	a := &scripted{replies: []reply{{502, "", `{"error":{"message":"Upstream HTTP/2 stream failed"}}`}}}
	b := &scripted{replies: []reply{{200, "text/event-stream", sse(append(append(slices.Clone(overloadedLead), unknown...), overloadedFailed)...)}}}
	c := &scripted{replies: []reply{{200, "text/event-stream", sse(
		`event: response.output_text.delta`+"\n"+`data: {"type":"response.output_text.delta","delta":"hello"}`,
		`event: response.completed`+"\n"+`data: {"type":"response.completed","response":{"id":"r1","status":"completed","output":[]}}`)}}}
	responsesOn(t, "a", a)
	responsesOn(t, "b", b)
	responsesOn(t, "c", c)
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"a/m", "b/m", "c/m"}, Routing: provider.Ordered, Patience: provider.PatienceOff}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, body := sendTo(s, "/v1/responses", codexAsk)
	if code != 200 || !strings.Contains(body, "hello") || strings.Contains(body, "server_is_overloaded") || strings.Contains(body, "keepalive") || c.n != 1 {
		t.Fatalf("%d %q (a %d, b %d, c %d)", code, body, a.n, b.n, c.n)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	if len(r.Tries) != 3 || r.Tries[1].Fail == "" {
		t.Fatalf("tries: %+v", r.Tries)
	}
}

// What only frames a reply, or isn't known, doesn't let a stream through;
// text, reasoning, a call or a reply's end does.
func TestStreamEventUnknownOnlyFrames(t *testing.T) {
	for _, x := range []struct {
		ev   string
		kind int
	}{
		{`data: {"type":"keepalive"}`, eventLead},
		{`event: response.rate_limits.updated` + "\n" + `data: {"type":"response.rate_limits.updated"}`, eventLead},
		{`data: {"type":"response.output_item.added","item":{"type":"compaction_summary"}}`, eventLead},
		{`data: {"x":1}`, eventLead},
		{`data: {}`, eventLead},
		{`data: {"type":"response.output_item.added","item":{"type":"web_search_call","id":"ws"}}`, eventContent},
		{`data: {"type":"response.output_item.added","item":{"type":"mcp_list_tools"}}`, eventContent},
		{`data: {"type":"response.web_search_call.searching","item_id":"ws"}`, eventContent},
		{`data: {"type":"response.function_call_arguments.delta","delta":"{"}`, eventContent},
		{`data: {"type":"response.custom_tool_call_input.delta","delta":"x"}`, eventContent},
		{`data: {"type":"response.output_text.annotation.added"}`, eventContent},
		{`data: {"type":"response.refusal.delta","delta":"no"}`, eventContent},
		{`event: content_block_start` + "\n" + `data: {"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"s","name":"web_search"}}`, eventContent},
		{`event: content_block_start` + "\n" + `data: {"type":"content_block_start","index":0,"content_block":{"type":"container_upload"}}`, eventLead},
		{`event: message_stop` + "\n" + `data: {"type":"message_stop"}`, eventContent},
		{`data: [DONE]`, eventContent},
	} {
		if k, _, _ := streamEvent([]byte(x.ev)); k != x.kind {
			t.Errorf("streamEvent(%s) = %d, want %d", x.ev, k, x.kind)
		}
	}
}

// A group in a group waits out busy members by its own Patience (#2410):
// the innermost that sets one, else the group asked for — same rule as
// FirstToken (#2246) and Sink (#2255). Before, replan always read the
// asked group's Waits, so an inner 10s under an outer 4s gave up at 4s.
// The pauses are 2s, 4s, 8s (retryPause 1s); each budget is 2s or more
// from where a pause ends, so a loaded runner's slower asks don't move a
// round across it.
func TestNestedGroupPatienceThroughOuter(t *testing.T) {
	overloaded := reply{529, "", `{"error":{"message":"Our servers are currently overloaded. Please try again later.","code":"server_is_overloaded"}}`}
	setup := func(t *testing.T, inner, outer int) *scripted {
		t.Helper()
		fresh(t)
		k := retryPause
		retryPause = time.Second
		t.Cleanup(func() { retryPause = k })
		a := &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
		b := &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
		scriptedOn(t, "a", provider.Chat, a)
		scriptedOn(t, "b", provider.Chat, b)
		if err := provider.SaveGroup(provider.Group{Name: "In", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Patience: inner}); err != nil {
			t.Fatal(err)
		}
		if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"group/in"}, Routing: provider.Ordered, Patience: outer}); err != nil {
			t.Fatal(err)
		}
		return a
	}
	ask := `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`

	// inner 10s under outer 4s: the inner budget allows a second replan
	// (2s+4s=6s < 10s, a third's 14s is past it); the outer's 4s would
	// have stopped after one (6s > 4s).
	a := setup(t, 10, 4)
	s := New()
	code, _ := postAs(t, s, "", ask)
	if a.n != 3 {
		t.Fatalf("asks: a.n=%d, want 3 — the inner group's 10s budget was not read", a.n)
	}
	if code != 529 {
		t.Fatalf("code=%d", code)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	var got int
	for _, try := range r.Tries {
		if try.Replan > 0 {
			got = try.Patience
		}
	}
	if got != 10 {
		t.Fatalf("trace patience=%d, want 10 (inner): %+v", got, r.Tries)
	}

	// direct to the inner group: same 10s budget, three asks
	fresh(t)
	k := retryPause
	retryPause = time.Second
	t.Cleanup(func() { retryPause = k })
	a = &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
	b := &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "In", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Patience: 10}); err != nil {
		t.Fatal(err)
	}
	code, _ = postAs(t, New(), "", `{"model":"group/in","messages":[{"role":"user","content":"hi"}]}`)
	if a.n != 3 || code != 529 {
		t.Fatalf("direct inner: a.n=%d code=%d", a.n, code)
	}
}

// An inner PatienceOff under an outer default must not replan (#2410):
// the inner Off is set, so it wins over the outer's 60s.
func TestNestedGroupPatienceInnerOff(t *testing.T) {
	fresh(t)
	overloaded := reply{529, "", `{"error":{"message":"Our servers are currently overloaded. Please try again later.","code":"server_is_overloaded"}}`}
	a := &scripted{replies: []reply{overloaded, overloaded}}
	b := &scripted{replies: []reply{overloaded, overloaded}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "In", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered, Patience: provider.PatienceOff}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"group/in"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	code, _ := postAs(t, New(), "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if code != 529 || a.n != 1 {
		t.Fatalf("inner Off through outer: code=%d a.n=%d (want 529, 1 — no replan)", code, a.n)
	}
}

// An inner that leaves Patience unset falls back to the asked group's (#2410).
func TestNestedGroupPatienceFallsBackToOuter(t *testing.T) {
	fresh(t)
	k := retryPause
	retryPause = time.Second
	t.Cleanup(func() { retryPause = k })
	overloaded := reply{529, "", `{"error":{"message":"Our servers are currently overloaded. Please try again later.","code":"server_is_overloaded"}}`}
	a := &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
	b := &scripted{replies: []reply{overloaded, overloaded, overloaded, overloaded}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	if err := provider.SaveGroup(provider.Group{Name: "In", Members: []string{"a/m", "b/m"}, Routing: provider.Ordered}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{Name: "G", Members: []string{"group/in"}, Routing: provider.Ordered, Patience: 4}); err != nil {
		t.Fatal(err)
	}
	s := New()
	code, _ := postAs(t, s, "", `{"model":"group/g","messages":[{"role":"user","content":"hi"}]}`)
	if a.n != 2 || code != 529 {
		t.Fatalf("unset inner, outer 4s: a.n=%d code=%d (want 2, 529)", a.n, code)
	}
	r := s.trace.routes[len(s.trace.routes)-1]
	var got int
	for _, try := range r.Tries {
		if try.Replan > 0 {
			got = try.Patience
		}
	}
	if got != 4 {
		t.Fatalf("trace patience=%d, want 4 (outer fallback): %+v", got, r.Tries)
	}
}
