package gateway

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log"
	"net/http"
	"net/http/httptest"
	"runtime/pprof"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/provider"
)

// An agent's request that magpie holds before routing it went nowhere a
// user could see (heyusaku6 on X): Codex thought for over ten minutes on a
// routing group's model, then on two others, the Routing page showed none
// of them, and only a restart of Codex and magpie ended it. Codex waits for
// a reply's headers with no limit of its own, and a request shows on the
// Routing page once a route is begun for it, so whatever held it before
// that (a lock, a wait with no end) held it in silence.
//
// What held it isn't known. So a request with no route begun for it is
// shown on the Routing page as routing once it has waited unroutedShow,
// and is answered with an error once it has waited unroutedMost, with
// magpie's log saying where every goroutine stood. Its context ends then
// too, so what waits on it lets go.
var unroutedShow, unroutedMost = 15 * time.Second, 2 * time.Minute

// unroutedWatched is whether a request to path is an agent's turn, which
// is given a route once magpie knows where it goes: Chat, Responses,
// Messages, Gemini and Codex's own backend. A compaction or a model list
// has no route of its own, and an MCP or a search call is not a turn.
func unroutedWatched(path string) bool {
	switch path {
	case "/v1/chat/completions", "/chat/completions", "/v1/responses", "/responses",
		"/v1/messages", "/messages", CodexPath + "/responses":
		return true
	}
	return strings.HasPrefix(path, "/v1beta/models/") && (strings.HasSuffix(path, ":generateContent") || strings.HasSuffix(path, ":streamGenerateContent"))
}

// unroutedProto is the API path's client speaks, for its error.
func unroutedProto(path string) provider.Protocol {
	switch {
	case strings.HasSuffix(path, "/messages"):
		return provider.Anthropic
	case strings.HasPrefix(path, "/v1beta/"):
		return provider.Gemini
	case strings.HasSuffix(path, "/responses"):
		return provider.Responses
	}
	return provider.Chat
}

// errUnrouted ends a request's context when magpie stopped holding it.
var errUnrouted = errors.New("magpie held the request too long before routing it")

// heldWriter is the client's writer while magpie may still answer for its
// handler: what the handler writes goes through it, and once magpie has
// answered (gaveUp) the handler's writes go nowhere. The handler's headers
// are its own until it writes, so magpie's answer never shares a map
// another goroutine is filling.
type heldWriter struct {
	w      http.ResponseWriter
	mu     sync.Mutex
	hdr    http.Header
	wrote  bool // the handler began its reply
	gaveUp bool // magpie answered for it
}

func newHeldWriter(w http.ResponseWriter) *heldWriter {
	return &heldWriter{w: w, hdr: http.Header{}}
}

func (h *heldWriter) Header() http.Header {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.wrote {
		return h.w.Header()
	}
	return h.hdr
}

// begin is the handler's reply starting: its headers go to the client's.
// Called with mu held; false once magpie has answered.
func (h *heldWriter) begin() bool {
	if h.gaveUp {
		return false
	}
	if !h.wrote {
		h.wrote = true
		out := h.w.Header()
		for k, vs := range h.hdr {
			out[k] = vs
		}
	}
	return true
}

func (h *heldWriter) WriteHeader(code int) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.begin() {
		h.w.WriteHeader(code)
	}
}

func (h *heldWriter) Write(b []byte) (int, error) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if !h.begin() {
		return 0, http.ErrHandlerTimeout
	}
	return h.w.Write(b)
}

func (h *heldWriter) Flush() { _ = h.FlushError() }

// FlushError is http.ResponseController's Flush, which looks for it
// before it unwraps.
func (h *heldWriter) FlushError() error {
	h.mu.Lock()
	defer h.mu.Unlock()
	if !h.begin() {
		return http.ErrHandlerTimeout
	}
	return http.NewResponseController(h.w).Flush()
}

// Unwrap is for http.ResponseController's deadlines.
func (h *heldWriter) Unwrap() http.ResponseWriter { return h.w }

// answer writes magpie's own reply, whole, unless the handler began one.
// Its length is said, so the client has all of it while the handler may
// still be held: a reply of no length ends only when the handler does.
func (h *heldWriter) answer(proto provider.Protocol, status int, msg string) bool {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.wrote || h.gaveUp {
		return false
	}
	h.gaveUp = true
	rec := httptest.NewRecorder()
	writeError(rec, proto, status, msg)
	out := h.w.Header()
	for k, vs := range rec.Header() {
		out[k] = vs
	}
	out.Set("Content-Length", strconv.Itoa(rec.Body.Len()))
	out.Set("Connection", "close")
	h.w.WriteHeader(status)
	h.w.Write(rec.Body.Bytes())
	_ = http.NewResponseController(h.w).Flush()
	return true
}

// unroutedHold is one watched request's watch: the route that shows it
// while it waits, and what ends it.
type unroutedHold struct {
	s      *Server
	r      *http.Request
	f      *inflight
	w      *heldWriter
	cancel context.CancelCauseFunc
	asked  atomic.Value // string: the model it asked for, once read
	shown  *Route       // under f.mu
	ended  bool         // under f.mu: answered by magpie
	over   bool         // under f.mu: its handler returned
	// masking counts the maskings under way for it: one held there waits
	// for Settings → Privacy's local model, and its error says so
	masking atomic.Int32
}

type unroutedKey struct{}

// noteAsked tells ctx's watch the model its request asked for, for the
// route that shows it.
func noteAsked(ctx context.Context, model string) {
	if u, _ := ctx.Value(unroutedKey{}).(*unroutedHold); u != nil {
		u.asked.Store(model)
	}
}

// noteMasking tells ctx's watch its request is being masked, until the
// func it answers is called.
func noteMasking(ctx context.Context) func() {
	u, _ := ctx.Value(unroutedKey{}).(*unroutedHold)
	if u == nil {
		return func() {}
	}
	u.masking.Add(1)
	return func() { u.masking.Add(-1) }
}

// watchUnrouted watches r, counted as f, until the func it answers is
// called as its handler returns.
func (s *Server) watchUnrouted(w http.ResponseWriter, r *http.Request, f *inflight) (http.ResponseWriter, *http.Request, func()) {
	if f == nil || !unroutedWatched(r.URL.Path) {
		return w, r, func() {}
	}
	ctx, cancel := context.WithCancelCause(r.Context())
	u := &unroutedHold{s: s, f: f, w: newHeldWriter(w), cancel: cancel}
	r = r.WithContext(context.WithValue(ctx, unroutedKey{}, u))
	u.r = r
	f.mu.Lock()
	f.unrouted = u
	f.mu.Unlock()
	show := time.AfterFunc(unroutedShow, u.show)
	most := time.AfterFunc(unroutedMost, u.giveUp)
	return u.w, r, func() {
		show.Stop()
		most.Stop()
		f.mu.Lock()
		// a timer that fired as the handler returned does nothing now
		u.over = true
		shown := u.shown
		if !u.ended {
			u.shown = nil
		}
		f.mu.Unlock()
		if shown != nil && !u.ended {
			// it was answered with no route of its own: it is the
			// usage log's, as before
			s.trace.drop(shown)
		}
		cancel(context.Canceled)
	}
}

// show begins the route that shows the request on the Routing page, as
// routing, while nothing else does.
func (u *unroutedHold) show() {
	f := u.f
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.begun) > 0 || u.shown != nil || u.ended || u.over {
		return
	}
	u.showLocked()
}

// showLocked begins the route that shows it; f.mu is held.
func (u *unroutedHold) showLocked() {
	asked, _ := u.asked.Load().(string)
	rt := Route{Time: u.f.start, Agent: callerOf(u.r).agent, Session: u.f.session, Model: asked, Provider: "magpie"}
	if rt.Agent == "" {
		rt.Agent = u.f.agent
	}
	rt.callerKey = access.Caller(u.r.Context()).KeyID
	u.shown = u.s.trace.begin(rt)
}

// routed is the request's own route begun: the one that showed it goes.
func (u *unroutedHold) routed() (drop *Route) {
	if u.ended {
		return nil
	}
	drop, u.shown = u.shown, nil
	return drop
}

// giveUp answers the request with an error, when no route was begun for it
// and its handler wrote nothing, ends its context, and says on its route
// and in the log what held it.
func (u *unroutedHold) giveUp() {
	f := u.f
	f.mu.Lock()
	if len(f.begun) > 0 || u.ended || u.over {
		f.mu.Unlock()
		return
	}
	held := time.Since(f.start).Round(time.Second)
	msg, where := unroutedMsg(held, u.masking.Load() > 0), ""
	if u.masking.Load() > 0 {
		where = " masking it"
	}
	if !u.w.answer(unroutedProto(u.r.URL.Path), http.StatusGatewayTimeout, msg) {
		f.mu.Unlock()
		return // the handler is answering
	}
	u.ended = true
	if u.shown == nil {
		u.showLocked()
	}
	shown := u.shown
	f.mu.Unlock()
	u.s.trace.update(shown, func(r *Route) {
		r.Done, r.Status, r.Error, r.Millis = true, http.StatusGatewayTimeout, msg, time.Since(r.Time).Milliseconds()
	})
	u.cancel(errUnrouted)
	asked, _ := u.asked.Load().(string)
	log.Printf("gateway: %s %s for %s from %s held %s before routing,%s; stopped with 504%s", u.r.Method, u.r.URL.Path, cmpOr(asked, "?"), cmpOr(f.agent, "?"), held, cmpOr(where, " not masking it"), unroutedStacks())
}

// unroutedMsg is the error of a request held for held before routing:
// one held while it was masked, with Settings → Privacy's local model on,
// waited for that model (#3682), and is told so, and how to send without
// waiting for it.
func unroutedMsg(held time.Duration, masking bool) string {
	if masking && modelOn() {
		why := ""
		if b := modelBusy(); b != "" {
			why = " (" + b + ")"
		}
		return fmt.Sprintf("magpie held this request for %s waiting for the local masking model (Settings → Privacy) to mask it%s, so it was stopped rather than sent masked by the rules alone. Send it again: what the model has already read is kept, so it goes quicker. To send requests without waiting for the model, turn it off in Settings → Privacy.", held, why)
	}
	if masking {
		return fmt.Sprintf("magpie held this request for %s while masking it (Settings → Privacy), before routing it to any provider, so it was stopped rather than left waiting. Send it again; if it keeps happening, restart magpie and report it with magpie's log, which says where it was held.", held)
	}
	return fmt.Sprintf("magpie held this request for %s without routing it to any provider, so it was stopped rather than left waiting. Send it again; if it keeps happening, restart magpie and report it with magpie's log, which says where it was held.", held)
}

// modelOn and modelBusy are internal/masking's, a test's
// to stand in for.
var modelOn, modelBusy = masking.ModelOn, masking.ModelBusy

func cmpOr(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

// unroutedDumped is when the goroutines were last written to the log for
// a held request: one dump says where, and a burst of held requests
// writes it once.
var unroutedDumped atomic.Int64

// unroutedStacks is every goroutine's stack, alike ones counted once, for
// the log at most once in ten minutes; "" otherwise.
func unroutedStacks() string {
	now := time.Now().UnixNano()
	last := unroutedDumped.Load()
	if last != 0 && now-last < int64(10*time.Minute) || !unroutedDumped.CompareAndSwap(last, now) {
		return ""
	}
	var b bytes.Buffer
	_ = pprof.Lookup("goroutine").WriteTo(&b, 1)
	const most = 256 << 10
	if b.Len() > most {
		b.Truncate(most)
	}
	return "; where every goroutine stood:\n" + b.String()
}
