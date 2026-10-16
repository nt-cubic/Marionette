package otlp

import (
	"crypto/rand"
	"encoding/hex"
	"strings"
	"time"

	"github.com/yetone/magpie/internal/usage"
)

// request is one gateway request the exporter watches: a root span, and
// a child for each routing attempt.
type request struct {
	x                                     *Exporter
	root                                  OTelSpan
	agent, session                        string
	sessionCandidate, decided, suppressed bool
}

// Decide once, before exporting any attempt, so a transcript discovered
// during the request cannot leave an exported attempt without its root.
func (t *request) skip() bool {
	if !t.decided {
		t.suppressed = t.sessionCandidate && t.x.SessionFresh(t.agent, t.session)
		t.decided = true
	}
	return t.suppressed
}

func otelID(size int) string {
	b := make([]byte, size)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// Accept W3C version 00 only; malformed or zero IDs start a fresh trace.
// Context is used locally and never forwarded with provider credentials.
func otelParent(value string) (traceID, parentID string) {
	parts := strings.Split(value, "-")
	if len(parts) != 4 || parts[0] != "00" || len(parts[1]) != 32 || len(parts[2]) != 16 || len(parts[3]) != 2 {
		return "", ""
	}
	for _, part := range parts[1:] {
		if strings.ToLower(part) != part {
			return "", ""
		}
		if _, err := hex.DecodeString(part); err != nil {
			return "", ""
		}
	}
	if parts[1] == strings.Repeat("0", 32) || parts[2] == strings.Repeat("0", 16) {
		return "", ""
	}
	return parts[1], parts[2]
}

// Begin watches a gateway request while the export is on. A local agent's
// own turn whose transcript the exporter already reads is left to the
// transcript.
func (x *Exporter) Begin(q usage.Request) usage.Watch {
	if !x.enabled() {
		return nil
	}
	candidate := q.Kind == "" && q.Plain
	if candidate && x.Session(q.Agent, q.Session) {
		return nil
	}
	traceID, parentID := otelParent(q.Header.Get("traceparent"))
	if traceID == "" {
		traceID = otelID(16)
	}
	return &request{x: x, root: OTelSpan{TraceID: traceID, SpanID: otelID(8), ParentID: parentID, Root: true}, agent: q.Agent, session: q.Session, sessionCandidate: candidate}
}

// End sends the root span, timed to now.
func (t *request) End(rec Record) {
	if t.skip() {
		return
	}
	root := t.root
	root.End = time.Now()
	t.x.offerSpan(rec, &root)
}

// Attempt sends an attempt as the root's child.
func (t *request) Attempt(rec Record) {
	if t.skip() {
		return
	}
	t.x.offerSpan(rec, &OTelSpan{TraceID: t.root.TraceID, SpanID: otelID(8), ParentID: t.root.SpanID})
}
