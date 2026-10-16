package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// The message is what #2400's agent showed, word for word; no vendor's
// reply was captured, so the envelopes around it below are each protocol's
// error shapes, not a vendor's bytes.
const highRiskMessage = "The request was rejected because it was considered high risk"

func TestHighRiskErrorMessage(t *testing.T) {
	for _, msg := range []string{highRiskMessage, " \n" + highRiskMessage + "\n"} {
		b, _ := json.Marshal(map[string]any{"error": map[string]any{"message": msg}})
		if note, ok := policyRefusal(b); !ok || !strings.Contains(note, highRiskMessage) {
			t.Errorf("not classified as a refusal: %s: %q", b, note)
		}
	}
	for _, msg := range []string{
		"high risk", "not " + highRiskMessage, highRiskMessage + "; please explain this message",
		`The server quoted "` + highRiskMessage + `"`, "invalid API key",
	} {
		b, _ := json.Marshal(map[string]any{"error": map[string]any{"message": msg}})
		if note, ok := policyRefusal(b); ok {
			t.Errorf("unrelated error classified as a refusal: %s: %q", b, note)
		}
	}
	for _, event := range []string{
		`data: {"error":{"message":"` + highRiskMessage + `"}}`,
		`event: error` + "\n" + `data: {"message":"` + highRiskMessage + `"}`,
		`data: {"type":"error","error":{"type":"invalid_request_error","message":"` + highRiskMessage + `"}}`,
		`data: {"type":"response.failed","response":{"error":{"message":"` + highRiskMessage + `"}}}`,
	} {
		if kind, status, msg := streamEvent([]byte(event)); kind == eventError || status != refusedStatus || !strings.Contains(msg, highRiskMessage) {
			t.Errorf("error event was not a refusal: %s: kind %d, status %d, message %q", event, kind, status, msg)
		}
	}
}

func TestHighRiskErrorFallsBack(t *testing.T) {
	for _, from := range []provider.Protocol{provider.Chat, provider.Responses, provider.Anthropic} {
		for _, wire := range []struct {
			name string
			code int
			body string
		}{
			{"http error", 400, `{"error":{"message":"` + highRiskMessage + `"}}`},
			{"stream error", 200, sse(`data: {"error":{"message":"` + highRiskMessage + `"}}`)},
		} {
			t.Run(string(from)+"/"+wire.name, func(t *testing.T) {
				fresh(t)
				ctype := "application/json"
				if wire.code == 200 {
					ctype = "text/event-stream"
				}
				a := &scripted{replies: []reply{{wire.code, ctype, wire.body}}}
				b := &scripted{replies: []reply{{200, "text/event-stream", anthropicAnswer}}}
				scriptedOn(t, "a", provider.Chat, a)
				scriptedOn(t, "b", provider.Anthropic, b)
				refusalGroup(t, "a/m", "b/m")
				path, ask := refusalErrorAsk(from)
				s := New()
				code, body := sendRefusalError(t, s, path, ask)
				if code != 200 || !strings.Contains(body, "from b") || strings.Contains(body, highRiskMessage) || a.n != 1 || b.n != 1 {
					t.Fatalf("status %d: %s (a %d, b %d)", code, body, a.n, b.n)
				}
				r := lastRoute(s)
				if len(r.Tries) != 2 || r.Tries[0].Fail != failRefused || r.Tries[0].Rest != nil || !strings.Contains(r.Tries[0].Error, highRiskMessage) {
					t.Fatalf("refusal was not recorded, or rested the account: %+v", r.Tries)
				}
			})
		}
	}
}

func refusalErrorAsk(from provider.Protocol) (string, string) {
	switch from {
	case provider.Responses:
		return "/v1/responses", codexAsk
	case provider.Anthropic:
		return "/v1/messages", `{"model":"group/g","max_tokens":10,"stream":true,"messages":[{"role":"user","content":"hi"}]}`
	default:
		return "/v1/chat/completions", `{"model":"group/g","stream":true,"messages":[{"role":"user","content":"hi"}]}`
	}
}

func sendRefusalError(t *testing.T, s *Server, path, body string) (int, string) {
	t.Helper()
	gw := httptest.NewServer(lanGuard(s.Handler()))
	defer gw.Close()
	client := gw.Client()
	client.Timeout = 10 * time.Second // deadlock guard for the held-open upstream
	res, err := client.Post(gw.URL+path, "application/json", strings.NewReader(body))
	if err != nil {
		t.Fatalf("request did not finish without upstream EOF: %v", err)
	}
	defer res.Body.Close()
	b, err := io.ReadAll(res.Body)
	if err != nil {
		t.Fatal(err)
	}
	return res.StatusCode, string(b)
}

// A provider's error ends this try even if its HTTP connection stays open.
// The server only ends when canceled or released by cleanup: the deadline
// is a deadlock guard, not an assertion about how fast a loaded runner is.
func TestRefusalErrorDoesNotWaitForEOF(t *testing.T) {
	for _, from := range []provider.Protocol{provider.Chat, provider.Responses, provider.Anthropic} {
		t.Run(string(from), func(t *testing.T) {
			fresh(t)
			release, canceled := make(chan struct{}), make(chan struct{})
			up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				io.ReadAll(r.Body)
				w.Header().Set("Content-Type", "text/event-stream")
				io.WriteString(w, sse(`data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":0}}`,
					`data: {"error":{"code":"content_filter","message":"The response was filtered"}}`))
				w.(http.Flusher).Flush()
				select {
				case <-r.Context().Done():
					close(canceled)
				case <-release:
				}
			}))
			defer up.Close()
			defer close(release)
			if err := provider.Save(provider.Provider{ID: "a", Name: "A", Key: "k", Models: []string{"m"}, Chat: up.URL + "/v1"}); err != nil {
				t.Fatal(err)
			}
			b := &scripted{replies: []reply{{200, "text/event-stream", anthropicAnswer}}}
			scriptedOn(t, "b", provider.Anthropic, b)
			refusalGroup(t, "a/m", "b/m")
			path, ask := refusalErrorAsk(from)
			s := New()
			code, body := sendRefusalError(t, s, path, ask)
			select {
			case <-canceled:
			case <-time.After(10 * time.Second):
				t.Fatal("a held refusal error waited for upstream EOF instead of canceling the try")
			}
			if code != 200 || !strings.Contains(body, "from b") || b.n != 1 {
				t.Fatalf("status %d: %s (b %d)", code, body, b.n)
			}
			if r := lastRoute(s); len(r.Tries) != 2 || r.Tries[0].Fail != failRefused || r.Tries[0].Rest != nil || r.Tries[0].Status != 400 || !strings.Contains(r.Tries[0].Error, "The response was filtered") || strings.Contains(r.Tries[0].Error, "context canceled") {
				t.Fatalf("cancellation changed refusal accounting: %+v", r.Tries)
			}
			if r := lastRoute(s); len(r.Usage) != 2 || r.Usage[0].Input != 3 {
				t.Fatalf("usage received before the error was lost: %+v", r.Usage)
			}
		})
	}
}

func TestHighRiskQuotationIsAnAnswer(t *testing.T) {
	for _, body := range []string{
		`{"id":"c1","choices":[{"message":{"role":"assistant","content":"` + highRiskMessage + `"},"finish_reason":"stop"}]}`,
		sse(`data: {"id":"c1","choices":[{"index":0,"delta":{"content":"`+highRiskMessage+`"}}]}`,
			`data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}`, `data: [DONE]`),
	} {
		fresh(t)
		ctype := "application/json"
		if strings.HasPrefix(body, "data:") {
			ctype = "text/event-stream"
		}
		a := &scripted{replies: []reply{{200, ctype, body}}}
		b := &scripted{replies: []reply{{200, "", chatOK}}}
		scriptedOn(t, "a", provider.Chat, a)
		scriptedOn(t, "b", provider.Chat, b)
		refusalGroup(t, "a/m", "b/m")
		s := New()
		path, ask := refusalErrorAsk(provider.Chat)
		code, got := sendRefusalError(t, s, path, ask)
		if code != 200 || strings.TrimSpace(got) != strings.TrimSpace(body) || a.n != 1 || b.n != 0 {
			t.Fatalf("quoted message was treated as a refusal: %d %s (a %d, b %d)", code, got, a.n, b.n)
		}
		if r := lastRoute(s); len(r.Tries) != 1 || r.Tries[0].Fail != "" {
			t.Fatalf("quoted message recorded as a failure: %+v", r.Tries)
		}
	}
}

func TestHighRiskErrorWhenRetryStops(t *testing.T) {
	for _, scenario := range []string{"no retries", "last candidate", "every candidate refuses"} {
		t.Run(scenario, func(t *testing.T) {
			fresh(t)
			if scenario == "no retries" {
				refusalRetries(t, 0)
			}
			errReply := reply{200, "text/event-stream", sse(`data: {"error":{"message":"` + highRiskMessage + `"}}`)}
			a, b := &scripted{replies: []reply{errReply}}, &scripted{replies: []reply{errReply}}
			scriptedOn(t, "a", provider.Chat, a)
			scriptedOn(t, "b", provider.Chat, b)
			members := []string{"a/m", "b/m"}
			if scenario == "last candidate" {
				members = members[:1]
			}
			refusalGroup(t, members...)
			s := New()
			path, ask := refusalErrorAsk(provider.Responses)
			code, body := sendRefusalError(t, s, path, ask)
			wantB := 0
			if scenario == "every candidate refuses" {
				wantB = 1
			}
			if code != 400 || !strings.Contains(body, highRiskMessage) || a.n != 1 || b.n != wantB {
				t.Fatalf("status %d: %s (a %d, b %d)", code, body, a.n, b.n)
			}
			for _, tr := range lastRoute(s).Tries {
				if tr.Fail != failRefused || tr.Rest != nil {
					t.Fatalf("refused candidate rested or changed error kind: %+v", tr)
				}
			}
		})
	}
}

func TestHighRiskTextAndStartedReplyAreNotReplayed(t *testing.T) {
	for _, content := range []string{
		`"content":"` + highRiskMessage + `"`,
		`"content":"The server says: ` + highRiskMessage + `"`,
		`"content":"half an answer"`,
		`"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}]`,
	} {
		t.Run(content, func(t *testing.T) {
			fresh(t)
			body := sse(`data: {"id":"c1","choices":[{"index":0,"delta":{`+content+`}}]}`,
				`data: {"error":{"message":"`+highRiskMessage+`"}}`, `data: [DONE]`)
			a := &scripted{replies: []reply{{200, "text/event-stream", body}}}
			b := &scripted{replies: []reply{{200, "", chatOK}}}
			scriptedOn(t, "a", provider.Chat, a)
			scriptedOn(t, "b", provider.Chat, b)
			refusalGroup(t, "a/m", "b/m")
			s := New()
			path, ask := refusalErrorAsk(provider.Chat)
			code, got := sendRefusalError(t, s, path, ask)
			if code != 200 || got != body || a.n != 1 || b.n != 0 {
				t.Fatalf("started reply changed or replayed: %d %s (a %d, b %d)", code, got, a.n, b.n)
			}
			if r := lastRoute(s); len(r.Tries) != 1 || r.Tries[0].Fail != failRefused || r.Tries[0].Rest != nil {
				t.Fatalf("late refusal rested the account: %+v", r.Tries)
			}
		})
	}
}

func TestHighRiskNamedErrorAfterContentRestsNobody(t *testing.T) {
	fresh(t)
	body := sse(`data: {"id":"c1","choices":[{"index":0,"delta":{"content":"half"}}]}`,
		"event: error\n"+`data: {"message":"`+highRiskMessage+`"}`, `data: [DONE]`)
	a := &scripted{replies: []reply{{200, "text/event-stream", body}}}
	b := &scripted{replies: []reply{{200, "", chatOK}}}
	scriptedOn(t, "a", provider.Chat, a)
	scriptedOn(t, "b", provider.Chat, b)
	refusalGroup(t, "a/m", "b/m")
	s := New()
	path, ask := refusalErrorAsk(provider.Chat)
	code, got := sendRefusalError(t, s, path, ask)
	if code != 200 || got != body || a.n != 1 || b.n != 0 {
		t.Fatalf("started reply changed or replayed: %d %s (a %d, b %d)", code, got, a.n, b.n)
	}
	if r := lastRoute(s); len(r.Tries) != 1 || r.Tries[0].Fail != failRefused || r.Tries[0].Rest != nil {
		t.Fatalf("named refusal was not recognized: %+v", r.Tries)
	}
}

func TestHighRiskErrorSplitAfterContent(t *testing.T) {
	const start = "data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n"
	for _, x := range []struct {
		name    string
		frame   string
		refused bool
	}{
		{"named error", sse("event: error\n" + `data: {"message":"` + highRiskMessage + `"}`), true},
		{"named coded error", sse("event: error\n" + `data: {"code":"content_filter","message":"filtered"}`), true},
		{"nested error", sse(`data: {"error":{"message":"` + highRiskMessage + `"}}`), true},
		{"quoted error", sse(`data: {"choices":[{"delta":{"content":"The error says: ` + highRiskMessage + `"}}]}`), false},
	} {
		t.Run(x.name, func(t *testing.T) {
			for split := 0; split <= len(x.frame); split++ {
				rec := httptest.NewRecorder()
				h := newHoldWriter(rec, true)
				h.Header().Set("Content-Type", "text/event-stream")
				stopped := false
				h.stop = func() { stopped = true }
				io.WriteString(h, start)
				io.WriteString(h, x.frame[:split])
				io.WriteString(h, x.frame[split:])
				if h.refusedAfter != x.refused || h.refused || stopped || !h.passing || rec.Body.String() != start+x.frame {
					t.Fatalf("split %d of %q: late refusal %v, held refusal %v, stopped %v, passing %v, body %q", split, x.frame, h.refusedAfter, h.refused, stopped, h.passing, rec.Body.String())
				}
			}
		})
	}
}
