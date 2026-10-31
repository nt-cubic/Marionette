package redact

import (
	"strings"
	"testing"
)

// The count Claude Code puts in a system reminder each turn is no secret,
// though the local model calls it one: steven_4422 on Discord (10-31) saw
// <total_tokens>{{SECRET_dkfgb3bf}} tokens left</total_tokens> on the
// Masking page, found by the local model. The tag's name has "token" in
// it, which secretCued took for the digits' name. The reporter's counts
// are as written, and the same counts at eight digits, which Claude Code
// sends on a large window (14902266) and the model's length check lets in.
func TestTokenCountIsNoSecret(t *testing.T) {
	for _, n := range []string{"1499872", "1493809", "14998720", "14938090"} {
		reminder := "<system-reminder>\n<total_tokens>" + n + " tokens left</total_tokens>\n</system-reminder>"
		f := &fakeModel{values: map[string]string{n: "secret"}}
		o := Options{Secrets: true, Personal: true, Model: f.find}
		body := `{"model":"claude-opus-4-1","system":[{"type":"text","text":"You are Claude Code."}],"messages":[{"role":"user","content":[{"type":"text","text":` + jsonQuote(reminder) + `},{"type":"text","text":"go on"}]}]}`
		if got, k := MaskJSON([]byte(body), o); k != 0 || string(got) != body {
			t.Errorf("%s: %d masked: %s", n, k, got)
		}
	}
	// a count in a usage object or a setting is no secret either (each
	// its own number: one masked is known, and masked wherever it is)
	for _, c := range []struct{ text, v string }{
		{`{"usage":{"total_tokens": 14998721}}`, "14998721"},
		{`max_tokens: 32000000`, "32000000"},
		{`token_count=14998722`, "14998722"},
		{`spin: 14998723`, "14998723"},
	} {
		f := &fakeModel{values: map[string]string{c.v: "secret"}}
		o := Options{Secrets: true, Model: f.find}
		if out, k := Mask(c.text, o); k != 0 {
			t.Errorf("%q: %d masked: %q", c.text, k, out)
		}
	}
}

// Digits the model calls a secret are masked where a secret's name gives
// them to it: a key's, a token's, a password's or a PIN's.
func TestNamedNumberSecretStillMasked(t *testing.T) {
	for _, c := range []struct{ text, v string }{
		{"api_key=48213097715", "48213097715"},
		{`{"token": "73914682055"}`, "73914682055"},
		{"the access token is 73914682055", "73914682055"},
		{"PIN: 73914682", "73914682"},
		{"user_pin=73914682", "73914682"},
		{"wifi password: 83920175", "83920175"},
		{"密码是83920175", "83920175"},
		{"<token>73914682055</token>", "73914682055"},
	} {
		f := &fakeModel{values: map[string]string{c.v: "secret"}}
		o := Options{Secrets: true, Model: f.find}
		if out, _ := Mask(c.text, o); strings.Contains(out, c.v) {
			t.Errorf("%q: not masked: %q", c.text, out)
		}
	}
}
