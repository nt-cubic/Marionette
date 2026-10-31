package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/usage"
)

// heldMasker masks nothing, and holds each request it is asked about
// until let is closed: masking that takes as long as the test says.
// entered is closed when the first one reaches it.
type heldMasker struct {
	let, entered chan struct{}
	once         *sync.Once
}

func (m heldMasker) hold(ctx context.Context) {
	m.once.Do(func() { close(m.entered) })
	select {
	case <-m.let:
	case <-ctx.Done():
	}
}

func (m heldMasker) JSONHitsContext(ctx context.Context, b []byte) ([]byte, []gwmw.Hit) {
	m.hold(ctx)
	return b, nil
}

func (m heldMasker) TextHitsContext(ctx context.Context, s string) (string, []gwmw.Hit) {
	m.hold(ctx)
	return s, nil
}

func (m heldMasker) JSONHits(b []byte) ([]byte, []gwmw.Hit) {
	return m.JSONHitsContext(context.Background(), b)
}

func (m heldMasker) TextHits(s string) (string, []gwmw.Hit) {
	return m.TextHitsContext(context.Background(), s)
}
func (m heldMasker) JSON(b []byte) ([]byte, int)   { b, _ = m.JSONHits(b); return b, 0 }
func (m heldMasker) Text(s string) (string, int)   { s, _ = m.TextHits(s); return s, 0 }
func (heldMasker) Restore(s string, _ bool) string { return s }
func (heldMasker) Value(string) (string, bool)     { return "", false }
func (heldMasker) Writer(w http.ResponseWriter) (http.ResponseWriter, func()) {
	return w, func() {}
}

// fremontdango on Discord: a Claude Code request sat on the Routing page
// as routing for a long while that it was being masked. While a
// request is masked its phase is "redact", on /v1/magpie/concurrency and
// on the route that shows it, and what masking took is kept, on its route
// and in its usage record, as a part of magpie's own time.
func TestMaskingIsAPhaseWithItsTime(t *testing.T) {
	unroutedLimits(t, 10*time.Millisecond, time.Minute)
	fresh(t)
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	slowOn(t, "a", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, chatStreamOK)
	}))
	withoutMasker(t)
	m := heldMasker{let: make(chan struct{}), entered: make(chan struct{}), once: new(sync.Once)}
	var letOnce sync.Once
	let := func() { letOnce.Do(func() { close(m.let) }) }
	t.Cleanup(gwmw.Mask(m))

	s := New()
	srv := httptest.NewServer(s.Handler())
	t.Cleanup(srv.Close)
	t.Cleanup(let) // before srv.Close, which waits for the request
	got := posted(srv, strings.NewReader(`{"model":"a/m","stream":true,"messages":[{"role":"user","content":"hi"}]}`))

	select {
	case <-m.entered:
	case r := <-got:
		t.Fatalf("answered %d %q %v before it was masked", r.code, r.body, r.err)
	case <-time.After(10 * time.Second):
		t.Fatal("the request never reached the masker")
	}
	entered := time.Now()
	// its phase, and the route that shows it as held, say masking
	deadline := time.Now().Add(10 * time.Second)
	for {
		phase, shown := "", ""
		for _, a := range s.Active() {
			phase = a.Phase
		}
		for _, r := range s.Trace(context.Background(), 0, 0).Routes {
			if !r.Done && r.Provider == "magpie" {
				shown = r.Phase
			}
		}
		if phase == phaseRedact && shown == phaseRedact {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("while masked: phase %q, its route's %q", phase, shown)
		}
		time.Sleep(5 * time.Millisecond)
	}
	held := time.Since(entered)
	let()
	var r unroutedReply
	select {
	case r = <-got:
	case <-time.After(15 * time.Second):
		t.Fatal("never answered once masked")
	}
	if r.err != nil || r.code != 200 || !strings.Contains(r.body, "from b") {
		t.Fatalf("answered %d %q %v", r.code, r.body, r.err)
	}

	rs := routesOf(s, "a/m")
	if len(rs) != 1 || !rs[0].Done || rs[0].Provider != "a" {
		t.Fatalf("routes: %+v", rs)
	}
	rt := rs[0]
	if rt.Phase != "" {
		t.Fatalf("a done route still says %q", rt.Phase)
	}
	if rt.Masking < held.Milliseconds() || rt.Masking > rt.Own || rt.Own == 0 {
		t.Fatalf("route: masking %d ms of own %d, held at least %s", rt.Masking, rt.Own, held)
	}
	recs := usage.Load(time.Time{})
	if len(recs) == 0 {
		t.Fatal("no usage record")
	}
	rec := recs[len(recs)-1]
	if rec.Masking != rt.Masking || rec.Own != rt.Own {
		t.Fatalf("record: masking %d own %d, route %d %d", rec.Masking, rec.Own, rt.Masking, rt.Own)
	}
}

// A request that isn't masked has no masking time, and never the phase.
func TestNoMaskingNoMaskingTime(t *testing.T) {
	fresh(t)
	slowOn(t, "a", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, chatStreamOK)
	}))
	s := New()
	code, body := postTo(t, s, "/v1/chat/completions", `{"model":"a/m","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code != 200 {
		t.Fatalf("%d %s", code, body)
	}
	rs := routesOf(s, "a/m")
	if len(rs) != 1 || rs[0].Masking != 0 || rs[0].Phase != "" || rs[0].Own == 0 {
		t.Fatalf("routes: %+v", rs)
	}
}

// lute on Discord: a masked request whose time magpie can't split (here
// the vendor refused it, so there is no first token) showed no masking
// time at all. What masking took is kept whenever it took any, all of
// it when magpie's own time is unknown.
func TestMaskingTimeKeptWhenOwnIsUnknown(t *testing.T) {
	fresh(t)
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	slowOn(t, "a", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusBadRequest)
		io.WriteString(w, `{"error":{"message":"no"}}`)
	}))
	withoutMasker(t)
	m := heldMasker{let: make(chan struct{}), entered: make(chan struct{}), once: new(sync.Once)}
	t.Cleanup(gwmw.Mask(m))
	const hold = 120 * time.Millisecond
	go func() {
		<-m.entered
		time.Sleep(hold)
		close(m.let)
	}()

	s := New()
	code, body := postTo(t, s, "/v1/chat/completions", `{"model":"a/m","stream":true,"messages":[{"role":"user","content":"hi"}]}`)
	if code == 200 {
		t.Fatalf("the vendor's refusal answered %d %s", code, body)
	}
	rs := routesOf(s, "a/m")
	if len(rs) != 1 || !rs[0].Done {
		t.Fatalf("routes: %+v", rs)
	}
	if rs[0].Own != 0 {
		t.Fatalf("own is known here (%d ms); this test is about when it isn't", rs[0].Own)
	}
	if rs[0].Masking < hold.Milliseconds() {
		t.Fatalf("masking %d ms, held %s", rs[0].Masking, hold)
	}
}
