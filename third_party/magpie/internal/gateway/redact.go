package gateway

import (
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
	m, err := gwmw.Masking(redactionWanted())
	if m == nil {
		return w, body, func() {}, err
	}
	type out struct {
		b []byte
		n int
	}
	o, err := gwmw.Safely(func() out { b, n := m.JSON(body); return out{b, n} })
	if err != nil || o.n == 0 {
		return w, body, func() {}, err
	}
	rw, done := m.Writer(w)
	return rw, o.b, done, nil
}

// redactedPrompt masks a parsed image or video prompt before any vendor
// body is built, including multipart forms, and restores the reply's text.
// It fails closed as redacted does.
func redactedPrompt(w http.ResponseWriter, prompt string) (http.ResponseWriter, string, func(), error) {
	m, masked, n, err := maskText(prompt)
	if err != nil || n == 0 {
		return w, prompt, func() {}, err
	}
	rw, done := m.Writer(w)
	return rw, masked, done, nil
}

// maskedText is s masked as the settings say, and how many values it
// swapped; an error when it can't be (fail closed).
func maskedText(s string) (string, int, error) {
	_, masked, n, err := maskText(s)
	return masked, n, err
}

// maskText is maskedText with the masker that masked it.
func maskText(s string) (gwmw.Masker, string, int, error) {
	m, err := gwmw.Masking(redactionWanted())
	if m == nil {
		return nil, s, 0, err
	}
	type out struct {
		s string
		n int
	}
	o, err := gwmw.Safely(func() out { t, n := m.Text(s); return out{t, n} })
	if err != nil {
		return nil, s, 0, err
	}
	return m, o.s, o.n, nil
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
	return masking.Wanted(settings.Load())
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
	masked, _, err := maskedText(query)
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
