// Package masking is Settings → Privacy's masking as the gateway's masker
// (gwmw.Masker): secrets, personal data and the user's own words go to a
// vendor as placeholders (internal/redact), and come back as they were.
// It reads the settings on each call. plugins/redact serves it.
package masking

import (
	"net/http"

	"github.com/yetone/magpie/internal/gwmw"
	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

// Masker masks as the settings say, read on each call.
type Masker struct{}

// Wanted says settings st ask for anything to be masked.
func Wanted(st settings.Settings) bool {
	return st.Redact || st.RedactPersonal || len(st.RedactWords) > 0
}

func options() redact.Options {
	st := settings.Shared()
	o := redact.Options{Secrets: st.Redact, Personal: st.RedactPersonal, Kinds: st.RedactKinds, Words: st.RedactWords, WordsAnyCase: st.RedactWordsAnyCase, Rules: st.RedactRules,
		Actions: st.RedactActions, Allow: st.RedactAllow}
	if st.RedactModel {
		o.Model = model
	}
	return o
}

// model is the local model (internal/pii) as redact's Model. One that
// can't answer panics, and the request is held back (gwmw.Safely) rather
// than sent with only the rules' masking: the user asked for the model's.
func model(texts []string) [][]redact.Found {
	got, err := pii.Find(texts)
	if err != nil {
		panic(err)
	}
	return got
}

func (Masker) JSON(body []byte) ([]byte, int) {
	o := options()
	if !o.Secrets && !o.Personal && len(o.Words) == 0 {
		return body, 0
	}
	return redact.MaskJSON(body, o)
}

func (Masker) Text(s string) (string, int) { return redact.Mask(s, options()) }

// JSONHits is JSON, with what it masked (gwmw.Teller).
func (Masker) JSONHits(body []byte) ([]byte, []gwmw.Hit) {
	o := options()
	if !o.Secrets && !o.Personal && len(o.Words) == 0 {
		return body, nil
	}
	var hits []gwmw.Hit
	o.Seen = told(&hits)
	out, _ := redact.MaskJSON(body, o)
	return out, hits
}

// TextHits is Text, with what it masked.
func (Masker) TextHits(s string) (string, []gwmw.Hit) {
	o := options()
	var hits []gwmw.Hit
	o.Seen = told(&hits)
	out, _ := redact.Mask(s, o)
	return out, hits
}

func (Masker) Value(p string) (string, bool) { return redact.Value(p) }

// told adds each value found to hits, with its hint: the value itself
// stays in redact's memory, or, sent as written, nowhere.
func told(hits *[]gwmw.Hit) func(redact.Hit) {
	return func(h redact.Hit) {
		*hits = append(*hits, gwmw.Hit{Kind: h.Kind, By: h.By, Placeholder: h.Placeholder, Action: h.Action, Head: h.Head, Len: h.Len})
	}
}

// Preview is s as the settings would send it, and what was found in it,
// with nothing remembered: no placeholder is kept for the replies, and
// none of its values is masked again later as one seen before. With
// masking off it is as masking would be turned on (Off says so), so the
// user can try it first.
func Preview(s string) (out string, hits []gwmw.Hit, off bool) {
	o := options()
	if !o.Secrets && !o.Personal && len(o.Words) == 0 {
		o.Secrets, o.Personal, off = true, true, true
	}
	o.Seen = told(&hits)
	out = redact.Preview(s, o)
	return out, hits, off
}

func (Masker) Restore(s string, escaped bool) string { return redact.Restore(s, escaped) }

func (Masker) Writer(w http.ResponseWriter) (http.ResponseWriter, func()) {
	rw := redact.NewWriter(w)
	return rw, rw.Finish
}

var _ gwmw.Teller = Masker{}
