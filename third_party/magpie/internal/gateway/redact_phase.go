package gateway

import (
	"context"
	"sync"
	"time"
)

// phaseRedact is the phase of a request while it is masked (redactedHits,
// maskTextHits): the rules, and Settings → Privacy's local model with its
// wait for a turn. Its route says it too (Route.Phase), so the Routing
// page's row says masking, not routing, while it lasts (fremontdango on
// Discord: a long Claude Code request sat as routing while it was
// being masked).
const phaseRedact = "redact"

// redaction is a request's maskings: how many are under way, since when
// one has been, the time spent in one before that, and the phase to put
// back when the last ends. It holds no text.
type redaction struct {
	mu      sync.Mutex
	now     int
	since   time.Time
	spent   time.Duration
	restore func()
}

// noteRedacting tells ctx's request it is being masked, until the func it
// answers is called: its phase is phaseRedact, the route that shows it
// says so, its watch counts the time toward neither of its limits
// (noteMasking), and the time is added to what masking took it
// (maskedFor). Maskings at once are counted once, from the first's start
// to the last's end.
func noteRedacting(ctx context.Context) func() {
	unmasked := noteMasking(ctx)
	f := inflightOf(ctx)
	if f == nil {
		return unmasked
	}
	f.redaction.mu.Lock()
	if f.redaction.now++; f.redaction.now == 1 {
		f.redaction.since = time.Now()
		f.redaction.restore = notePhase(ctx, phaseRedact)
	}
	f.redaction.mu.Unlock()
	f.showRedacting()
	var once sync.Once
	return func() {
		once.Do(func() {
			f.redaction.mu.Lock()
			var restore func()
			if f.redaction.now--; f.redaction.now == 0 {
				f.redaction.spent += time.Since(f.redaction.since)
				restore, f.redaction.restore = f.redaction.restore, nil
			}
			f.redaction.mu.Unlock()
			if restore != nil {
				restore()
			}
			unmasked()
			f.showRedacting()
		})
	}
}

// redacting says whether f is being masked now.
func (f *inflight) redacting() bool {
	f.redaction.mu.Lock()
	defer f.redaction.mu.Unlock()
	return f.redaction.now > 0
}

// maskedFor is how long masking has taken f so far, the one under way
// included; 0 for none or no f.
func (f *inflight) maskedFor() time.Duration {
	if f == nil {
		return 0
	}
	f.redaction.mu.Lock()
	defer f.redaction.mu.Unlock()
	d := f.redaction.spent
	if f.redaction.now > 0 {
		d += time.Since(f.redaction.since)
	}
	return d
}

// maskingMs is the ms of own, magpie's own time before the request went
// out (splitSent), that went on masking it: what masking took it, at
// most own. Under a ms is still said, as 1; none is 0. With own unknown
// (0: a plugin's fetch or Claude Code sent it, or no first token came)
// it is all masking took: the request was masked all the same, and the
// Routing page showed no masking for it (Discord, lute).
func maskingMs(ctx context.Context, own int64) int64 {
	d := inflightOf(ctx).maskedFor()
	if d <= 0 {
		return 0
	}
	ms := max(d.Milliseconds(), 1)
	if own > 0 {
		return min(ms, own)
	}
	return ms
}

// showRedacting puts f's phase on the route that shows it, its own or the
// one its watch began (unroutedHold.show), while it isn't done.
func (f *inflight) showRedacting() {
	rt := f.route.Load()
	f.mu.Lock()
	tr := f.tr
	if rt == nil && f.unrouted != nil {
		rt = f.unrouted.shown
	}
	f.mu.Unlock()
	if rt == nil || tr == nil {
		return
	}
	tr.update(rt, func(r *Route) {
		if r.Done {
			r.Phase = ""
			return
		}
		r.Phase = ""
		if f.redacting() {
			r.Phase = phaseRedact
		}
	})
}
