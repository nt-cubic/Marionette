package gateway

import (
	"errors"
	"fmt"
	"net/http"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/masking"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// redacted masks a request's secrets, as the settings say, before it goes
// to a vendor, and wraps w so what the vendor answers has them back; done
// writes what the wrapper still holds. Nothing masked, nothing wrapped. It
// fails closed: masking wanted with no masker serving, or a masker that
// fails, is an error, and the request goes nowhere (gwmw.Masking).
func redacted(w http.ResponseWriter, body []byte) (http.ResponseWriter, []byte, func(), error) {
	w, body, done, _, err := redactedHits(w, body)
	return w, body, done, err
}

// redactedHits is redacted with what it swapped, for the record of it
// (noteMasked).
func redactedHits(w http.ResponseWriter, body []byte) (http.ResponseWriter, []byte, func(), maskHits, error) {
	m, err := gwmw.Masking(redactionWanted())
	if m == nil {
		return w, body, func() {}, maskHits{}, err
	}
	type out struct {
		b []byte
		h maskHits
	}
	o, err := gwmw.Safely(func() out {
		if t, ok := m.(gwmw.Teller); ok {
			b, hits := t.JSONHits(body)
			return out{b, hitsOf(hits)}
		}
		b, n := m.JSON(body)
		return out{b, maskHits{n: n}}
	})
	if err == nil {
		err = blocked(o.h)
	}
	if err != nil {
		return w, body, func() {}, maskHits{}, err
	}
	if o.h.n == 0 { // only values sent as written: told, nothing to put back
		return w, body, func() {}, o.h, nil
	}
	rw, done := m.Writer(w)
	return rw, o.b, done, o.h, nil
}

// redactedPrompt masks a parsed image or video prompt before any vendor
// body is built, including multipart forms, and restores the reply's text.
// It fails closed as redacted does.
func redactedPrompt(w http.ResponseWriter, prompt string) (http.ResponseWriter, string, func(), error) {
	w, masked, done, _, err := redactedPromptHits(w, prompt)
	return w, masked, done, err
}

// redactedPromptHits is redactedPrompt with what it swapped.
func redactedPromptHits(w http.ResponseWriter, prompt string) (http.ResponseWriter, string, func(), maskHits, error) {
	m, masked, h, err := maskTextHits(prompt)
	if err != nil {
		return w, prompt, func() {}, maskHits{}, err
	}
	if h.n == 0 {
		return w, prompt, func() {}, h, nil
	}
	rw, done := m.Writer(w)
	return rw, masked, done, h, nil
}

// maskedText is s masked as the settings say, and how many values it
// swapped; an error when it can't be (fail closed).
func maskedText(s string) (string, int, error) {
	_, masked, h, err := maskTextHits(s)
	return masked, h.n, err
}

// maskTextHits is maskedText with the masker that masked it and what it
// swapped.
func maskTextHits(s string) (gwmw.Masker, string, maskHits, error) {
	m, err := gwmw.Masking(redactionWanted())
	if m == nil {
		return nil, s, maskHits{}, err
	}
	type out struct {
		s string
		h maskHits
	}
	o, err := gwmw.Safely(func() out {
		if t, ok := m.(gwmw.Teller); ok {
			got, hits := t.TextHits(s)
			return out{got, hitsOf(hits)}
		}
		got, n := m.Text(s)
		return out{got, maskHits{n: n}}
	})
	if err == nil {
		err = blocked(o.h)
	}
	if err != nil {
		return nil, s, maskHits{}, err
	}
	return m, o.s, o.h, nil
}

// hitsOf are a Teller's hits, counting the places masked: a value the
// user set to warn about goes as written, so it is told, not counted.
func hitsOf(hits []gwmw.Hit) maskHits {
	h := maskHits{hits: hits}
	for _, x := range hits {
		if x.Action != "warn" {
			h.n++
		}
	}
	return h
}

// maskBlocked is a request held back: it has a value of a kind the user
// set to block (Settings › Privacy). It names the kind and the value's
// hint, never the value.
type maskBlocked struct{ hit gwmw.Hit }

func (e maskBlocked) Error() string {
	one := fmt.Sprintf("%d characters", e.hit.Len)
	if e.hit.Head != "" {
		one = e.hit.Head + "…, " + one
	}
	return fmt.Sprintf("magpie held this request back: Settings › Privacy blocks %s, and it has one (%s). Nothing was sent.", e.hit.Kind, one)
}

// blocked is the error for hits with a value to hold the request back
// for, recorded as a request held back; nil without one.
func blocked(h maskHits) error {
	for _, x := range h.hits {
		if x.Action == "block" {
			noteBlocked(h)
			return maskBlocked{x}
		}
	}
	return nil
}

// maskStatus is the status a request masking failed for gets: 400 for
// one held back, which sent again is held back again, 503 for a masker
// that couldn't mask it.
func maskStatus(err error) int {
	if errors.As(err, new(maskBlocked)) {
		return http.StatusBadRequest
	}
	return http.StatusServiceUnavailable
}

// restored is s with what a masker masked before put back; s when none
// serves.
func restored(s string, escaped bool) string {
	if m, ok := gwmw.Served(); ok {
		return m.Restore(s, escaped)
	}
	return s
}

// restoring wraps w so values masked before come back, when a masker
// serves.
func restoring(w http.ResponseWriter) (http.ResponseWriter, func()) {
	if m, ok := gwmw.Served(); ok {
		return m.Writer(w)
	}
	return w, func() {}
}

// redactionWanted: the settings ask for something to be masked. The
// masking itself is served by the redact plugin (plugins/redact); this is
// the core's own reading, so a request isn't sent unmasked when the plugin
// isn't there to mask it.
func redactionWanted() bool {
	return masking.Wanted(settings.Shared())
}

// maskedQuery masks a web search's query, as the settings say, before it
// goes to a search service that isn't a model's endpoint: a search API, or
// a Kimi Code plan's or WorkBuddy's search (to p, nil for an API; one the
// user set to go unmasked keeps it as it is). A model's web_search writes
// its query from a request already masked, but an agent's call to the
// search MCP carries it as the agent's model wrote it, values put back
// (lc on Discord). It fails closed as redacted does.
func maskedQuery(query string, p *provider.Provider) (string, error) {
	if p != nil && p.SkipsRedaction() {
		return query, nil
	}
	_, masked, h, err := maskTextHits(query)
	if err == nil {
		id := ""
		if p != nil {
			id = p.ID
		}
		noteMasked(nil, "search", h, masked, 0, id, "")
	}
	return masked, err
}

// unredactedRoute says a request resolved to p, or to the group whose
// models are ms, goes only where the user set requests to go unmasked
// (provider.SkipsRedaction): p and each of its fallbacks, or every model of
// the group (a member's own fallbacks are not the group's, see planGroup).
// One that may go on to anyone else stays masked for all of them.
func unredactedRoute(p provider.Provider, isGroup bool, ms []provider.Member) bool {
	if isGroup {
		if len(ms) == 0 {
			return false
		}
		for _, m := range ms {
			if !m.Provider.SkipsRedaction() {
				return false
			}
		}
		return true
	}
	if !p.SkipsRedaction() {
		return false
	}
	for _, id := range p.Fallback {
		// one that resolves to nothing isn't tried (plan)
		if fp, _, ok := provider.Resolve(id); ok && !fp.SkipsRedaction() {
			return false
		}
	}
	return true
}
