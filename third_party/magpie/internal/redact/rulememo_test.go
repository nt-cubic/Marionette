package redact

import (
	"strings"
	"testing"
	"time"
)

func resetRuleMemo() {
	ruleMemo.Lock()
	ruleMemo.cur, ruleMemo.old = nil, nil
	ruleMemo.Unlock()
}

// TestRulesRunOnceAText: a history masked for a vendor again on the next
// turn has the rules run only over what it did not have before (#4353),
// and comes out as it did.
func TestRulesRunOnceAText(t *testing.T) {
	Forget()
	resetRuleMemo()
	o := Options{Secrets: true, Personal: true, Route: "codex"}
	first := codexHistory(40)
	want, _ := MaskJSON(first, o)
	runs := ruleRuns.Load()
	next := codexHistory(41)
	got, _ := MaskJSON(next, o)
	// the new turn's one long string, its tool's output
	if n := ruleRuns.Load() - runs; n != 1 {
		t.Fatalf("the rules ran over %d texts for one new turn", n)
	}
	if !strings.HasPrefix(string(got), string(want[:len(want)-len(`],"stream":true}`)])) {
		t.Fatal("the history masked again is not what it was")
	}
	Forget()
	resetRuleMemo()
	if again, _ := MaskJSON(next, o); string(again) != string(got) {
		t.Fatal("remembered and fresh rule finds differ")
	}
}

// TestRuleMemoKeysWhatTheRulesRead: what the rules found is not taken for
// a text read under other options, the user's own rules or another day.
func TestRuleMemoKeysWhatTheRulesRead(t *testing.T) {
	defer func(c func() time.Time) { dateClock = c }(dateClock)
	dateClock = func() time.Time { return time.Date(2026, 10, 24, 9, 30, 0, 0, time.Local) }
	resetRuleMemo()
	text := strings.Repeat("filler words with nothing in them ", 8) + "DOB: 03/05/1990, mail rkato@example.org, token ACME-7f3k9q2w8e4r6t1y"
	mine := []Rule{{Kind: "ACME", Prefix: "ACME-", Regex: `ACME-[0-9a-z]{16}`}}
	kinds := func(o Options) string {
		var out []string
		for _, sp := range ruleSpans(text, o) {
			out = append(out, sp.kind)
		}
		return strings.Join(out, ",")
	}
	secrets := kinds(Options{Secrets: true})
	all := kinds(Options{Secrets: true, Personal: true})
	if secrets == all {
		t.Fatalf("personal data on finds what secrets alone do: %q", all)
	}
	if got := kinds(Options{Secrets: true}); got != secrets {
		t.Fatalf("the same options found %q, then %q", secrets, got)
	}
	if got := kinds(Options{Secrets: true, Rules: mine}); !strings.Contains(got, "ACME") {
		t.Fatalf("the user's own rule went unread: %q", got)
	}
	off := kinds(Options{Secrets: true, Personal: true, Kinds: map[string]bool{"birthday": false}})
	if strings.Contains(off, "BIRTHDAY") || !strings.Contains(all, "BIRTHDAY") {
		t.Fatalf("birthdays turned off still found (%q; with them on %q)", off, all)
	}
	// the day after the date: no birthday, a date an agent runs on
	dateClock = func() time.Time { return time.Date(1990, 3, 6, 9, 30, 0, 0, time.Local) }
	if got := kinds(Options{Secrets: true, Personal: true}); strings.Contains(got, "BIRTHDAY") {
		t.Fatalf("a date of yesterday found as a birthday, as it was 36 years on: %q", got)
	}
}
