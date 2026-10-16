package gateway

import (
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/usage"
)

// fakeObserver watches every request when watch is set, and keeps what it
// was told.
type fakeObserver struct {
	watch  bool
	bodies usage.Bodies

	mu       sync.Mutex
	begun    []usage.Request
	attempts []usage.Record
	ends     []usage.Record
	rows     []usage.Record
}

func (f *fakeObserver) Serve() func()        { return func() {} }
func (f *fakeObserver) Bodies() usage.Bodies { return f.bodies }
func (f *fakeObserver) Begin(q usage.Request) usage.Watch {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.begun = append(f.begun, q)
	if !f.watch {
		return nil
	}
	return fakeWatch{f}
}
func (f *fakeObserver) Ledger(r usage.Record) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.rows = append(f.rows, r)
}

type fakeWatch struct{ f *fakeObserver }

func (w fakeWatch) Attempt(r usage.Record) {
	w.f.mu.Lock()
	defer w.f.mu.Unlock()
	w.f.attempts = append(w.f.attempts, r)
}
func (w fakeWatch) End(r usage.Record) {
	w.f.mu.Lock()
	defer w.f.mu.Unlock()
	w.f.ends = append(w.f.ends, r)
}

// The observer point: each observer is asked about each request with what
// the gateway knows of it; one that watches is told each attempt and the
// end, with the bodies it wants, and the ledger row says it was watched;
// one that doesn't is told nothing more, and its row isn't marked.
func TestObserverPoint(t *testing.T) {
	big := strings.Repeat("A", 300<<10)
	for _, c := range []struct {
		name   string
		watch  bool
		bodies usage.Bodies
		stream bool
	}{
		{"unwatched", false, usage.WholeBodies, false},
		{"no bodies", true, usage.NoBodies, false},
		{"captured", true, usage.CapturedBodies, false},
		{"whole", true, usage.WholeBodies, false},
		{"whole streamed", true, usage.WholeBodies, true},
	} {
		t.Run(c.name, func(t *testing.T) {
			f := &fake{ctype: "application/json", reply: `{"id":"c1","model":"m1","choices":[{"message":{"role":"assistant","content":"` + big + `"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}`}
			if c.stream {
				f.ctype = "text/event-stream"
				f.reply = "data: {\"choices\":[{\"delta\":{\"content\":\"" + big + "\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\ndata: [DONE]\n\n"
			}
			setup(t, provider.Chat, f)
			o := &fakeObserver{watch: c.watch, bodies: c.bodies}
			defer usage.Observe(o)()
			req := httptest.NewRequest("POST", "/v1/chat/completions", strings.NewReader(`{"model":"m1","messages":[{"role":"user","content":"hello `+big+`"}]}`))
			req.RemoteAddr = "127.0.0.1:1234"
			req.Header.Set("User-Agent", "pi/1.0")
			req.Header.Set("X-Session-Id", "s1")
			w := httptest.NewRecorder()
			New().Handler().ServeHTTP(w, req)
			if w.Code != 200 {
				t.Fatalf("gateway: %d", w.Code)
			}
			o.mu.Lock()
			defer o.mu.Unlock()
			if len(o.begun) != 1 {
				t.Fatalf("begun %d, want 1", len(o.begun))
			}
			if q := o.begun[0]; q.Agent != "pi" || q.Session != "s1" || !q.Plain || q.Kind != "" {
				t.Fatalf("request: %+v", q)
			}
			if len(o.rows) != 1 || o.rows[0].Watched != c.watch {
				t.Fatalf("ledger rows: %d, watched %v", len(o.rows), len(o.rows) > 0 && o.rows[0].Watched)
			}
			if !c.watch {
				if len(o.attempts)+len(o.ends) != 0 {
					t.Fatal("an unwatched request was told on")
				}
				return
			}
			if len(o.attempts) != 1 || len(o.ends) != 1 {
				t.Fatalf("attempts %d, ends %d; want 1, 1", len(o.attempts), len(o.ends))
			}
			a := o.attempts[0]
			if a.Model != "m1" || a.Provider != "fake" || a.Input != 10 || a.Output != 5 || a.Status != 200 || o.ends[0].Status != 200 {
				t.Fatalf("attempt %+v, end %+v", a, o.ends[0])
			}
			switch c.bodies {
			case usage.NoBodies:
				if a.BodyIn != "" || a.BodyOut != "" {
					t.Fatal("bodies sent to an observer that wants none")
				}
			case usage.CapturedBodies:
				if !strings.Contains(a.BodyIn, "hello") || !strings.HasSuffix(a.BodyOut, usage.BodyCut) {
					t.Fatalf("captured bodies: in=%d out=%d", len(a.BodyIn), len(a.BodyOut))
				}
			case usage.WholeBodies:
				if strings.Contains(a.BodyIn, usage.BodyCut) || strings.Contains(a.BodyOut, usage.BodyCut) || len(a.BodyIn) < len(big) || len(a.BodyOut) < len(big) {
					t.Fatalf("whole bodies: in=%d out=%d", len(a.BodyIn), len(a.BodyOut))
				}
			}
		})
	}
}
