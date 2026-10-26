package redact_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/redact"
)

// The masking evaluation: made-up texts of the kinds agents send (prose,
// code, logs, tool results, JSON), each with the values that must be
// masked and those that must go as written. The cases (eval_cases_test.go)
// start from what users reported masked wrongly, beside the values the
// masking is for. Nothing in them is anyone's data.
//
// Three ways are scored on them:
//   - rules: the rules alone, the model off;
//   - model: what the local model found, as it said it, before any check;
//   - both:  the rules and the model with magpie's checks on what it found,
//     as a user with the model on gets.
//
// What the model said is recorded in testdata/eval-model.json, so the
// test runs the whole of "both" without the model or the network, and
// fails when a value that came out right comes out wrong. A value known
// to come out wrong says so (evalValue.Known, an issue), and fails once
// it comes out right, to have its Known taken off.
//
// To ask the real model again, after a case is added or a new model,
// point MAGPIE_EVAL_CACHE at the cache folder it was downloaded into
// (the folder that holds magpie/privacy-filter):
//
//	MAGPIE_EVAL_CACHE=$HOME/.cache go test -run TestMaskEval -v ./internal/redact
//
// which writes the recording again and logs the table.
func TestMaskEval(t *testing.T) {
	rec := readRecording(t)
	if dir := os.Getenv("MAGPIE_EVAL_CACHE"); dir != "" {
		rec = askModel(t, dir)
		writeRecording(t, rec)
	}
	var stale []string
	for _, c := range evalCases {
		if r, ok := rec[c.ID]; !ok || r.Text != textHash(c.Text) {
			stale = append(stale, c.ID)
		}
	}
	if len(stale) > 0 {
		t.Fatalf("the model's recording is not of cases %v; record it again with MAGPIE_EVAL_CACHE (see TestMaskEval)", stale)
	}
	replay := func(texts []string) [][]redact.Found {
		out := make([][]redact.Found, len(texts))
		for i, s := range texts {
			for _, c := range evalCases {
				if c.Text == s {
					out[i] = rec[c.ID].Found
				}
			}
		}
		return out
	}

	redact.Forget()
	var tally evalTally
	for _, c := range evalCases {
		rules := redact.Preview(c.Text, redact.Options{Secrets: true, Personal: true})
		both := redact.Preview(c.Text, redact.Options{Secrets: true, Personal: true, Model: replay})
		for _, v := range c.Mask {
			checkValue(t, c, v, true, &tally, gone(rules, v.V), modelFound(c.Text, v.V, rec[c.ID].Found), gone(both, v.V))
		}
		for _, v := range c.Keep {
			checkValue(t, c, v, false, &tally, gone(rules, v.V), modelFound(c.Text, v.V, rec[c.ID].Found), gone(both, v.V))
		}
	}
	t.Log("\n" + tally.table())
}

// checkValue counts one value in each way, and holds "both" to what the
// case says of it.
func checkValue(t *testing.T, c evalCase, v evalValue, mask bool, tally *evalTally, rules, model, both bool) {
	t.Helper()
	tally.add(v.Kind, mask, rules, model, both)
	if rules != mask || model != mask || both != mask {
		tally.wrong = append(tally.wrong, fmt.Sprintf("%-18s %-6s %-7s %-5s %-5s %-5s %q",
			c.ID, map[bool]string{true: "mask", false: "keep"}[mask], v.Kind, mark(rules == mask), mark(model == mask), mark(both == mask), v.V))
	}
	if v.Known != "" {
		if both == mask {
			t.Errorf("%s: %q now comes out right; take its Known (%s) off", c.ID, v.V, v.Known)
		}
		return
	}
	switch {
	case mask && !both:
		t.Errorf("%s: %s %q went as written", c.ID, v.Kind, v.V)
	case !mask && both:
		t.Errorf("%s: %q was masked, and should go as written", c.ID, v.V)
	}
}

// placeholders are what masking puts in a value's place; a made-up key
// keeps no trace of the value, so the value is gone if it isn't there.
var placeholders = regexp.MustCompile(`\{\{[A-Z_]+_[a-z2-7]{8}\}\}`)

// gone says v is not in out as it is: masked, or a part of it masked.
// The placeholders are taken out first, since their suffix may spell v.
func gone(out, v string) bool {
	return !strings.Contains(placeholders.ReplaceAllString(out, "\x00"), v)
}

// modelFound says the model put a span of a kind magpie masks over some
// of v, where v is in text.
func modelFound(text, v string, fs []redact.Found) bool {
	for at := 0; ; {
		i := strings.Index(text[at:], v)
		if i < 0 {
			return false
		}
		a, b := at+i, at+i+len(v)
		for _, f := range fs {
			if redact.ModelKinds[f.Label] != "" && f.Start < b && f.End > a {
				return true
			}
		}
		at = b
	}
}

// evalTally counts, by kind, the values each way masked: of those to
// mask (found), and of those to keep (wrongly).
type evalTally struct {
	kinds map[string]*evalCount
	wrong []string // the values some way got wrong
}

func mark(right bool) string {
	if right {
		return "ok"
	}
	return "WRONG"
}

type evalCount struct {
	mask, keep   int
	found, wrong [3]int // rules, model, both
}

func (t *evalTally) add(kind string, mask bool, got ...bool) {
	if t.kinds == nil {
		t.kinds = map[string]*evalCount{}
	}
	n := t.kinds[kind]
	if n == nil {
		n = &evalCount{}
		t.kinds[kind] = n
	}
	if mask {
		n.mask++
	} else {
		n.keep++
	}
	for i, g := range got {
		if g && mask {
			n.found[i]++
		} else if g {
			n.wrong[i]++
		}
	}
}

// table is the tally: for each kind, how many of the values to mask each
// way masked, and how many of those to keep it masked all the same.
func (t *evalTally) table() string {
	var ks []string
	all := &evalCount{}
	for k, n := range t.kinds {
		ks = append(ks, k)
		all.mask += n.mask
		all.keep += n.keep
		for i := range 3 {
			all.found[i] += n.found[i]
			all.wrong[i] += n.wrong[i]
		}
	}
	sort.Strings(ks)
	var b strings.Builder
	fmt.Fprintf(&b, "%-10s %18s   %18s   %18s\n", "", "rules", "model", "both")
	fmt.Fprintf(&b, "%-10s %8s %9s   %8s %9s   %8s %9s\n", "kind", "masked", "wrongly", "masked", "wrongly", "masked", "wrongly")
	line := func(k string, n *evalCount) {
		fmt.Fprintf(&b, "%-10s", k)
		for i := range 3 {
			fmt.Fprintf(&b, " %8s %9s  ", frac(n.found[i], n.mask), frac(n.wrong[i], n.keep))
		}
		b.WriteString("\n")
	}
	for _, k := range ks {
		line(k, t.kinds[k])
	}
	line("all", all)
	fmt.Fprintf(&b, "\n%-18s %-6s %-7s %-5s %-5s %-5s %s\n", "case", "should", "kind", "rules", "model", "both", "value")
	for _, w := range t.wrong {
		b.WriteString(w + "\n")
	}
	return b.String()
}

func frac(n, of int) string {
	if of == 0 {
		return "-"
	}
	return fmt.Sprintf("%d/%d", n, of)
}

// ---- the recording ----------------------------------------------------------

const recordingPath = "testdata/eval-model.json"

// recorded is what the model found in one case's text, and the text's
// hash, to tell a recording made of another text.
type recorded struct {
	Text  string         `json:"text"`
	Found []redact.Found `json:"found"`
}

func textHash(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:8])
}

func readRecording(t *testing.T) map[string]recorded {
	rec := map[string]recorded{}
	b, err := os.ReadFile(recordingPath)
	if os.IsNotExist(err) {
		return rec
	}
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(b, &rec); err != nil {
		t.Fatalf("%s: %v", recordingPath, err)
	}
	return rec
}

func writeRecording(t *testing.T, rec map[string]recorded) {
	b, err := json.MarshalIndent(rec, "", "\t")
	if err != nil {
		t.Fatal(err)
	}
	tmp := recordingPath + ".tmp"
	if err := os.WriteFile(tmp, append(b, '\n'), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(tmp, recordingPath); err != nil {
		t.Fatal(err)
	}
}

// askModel runs every case's text through the real model, downloaded
// into the cache folder dir, one text at a time, as a request's text
// that has the model to itself is.
func askModel(t *testing.T, dir string) map[string]recorded {
	t.Setenv("XDG_CACHE_HOME", dir)
	if !pii.Installed() {
		t.Fatalf("the model isn't in %s", filepath.Join(dir, "magpie", "privacy-filter"))
	}
	defer pii.Stop()
	rec := map[string]recorded{}
	for _, c := range evalCases {
		got, err := pii.Find([]string{c.Text})
		if err != nil {
			t.Fatalf("%s: %v", c.ID, err)
		}
		fs := got[0]
		if fs == nil {
			fs = []redact.Found{}
		}
		rec[c.ID] = recorded{Text: textHash(c.Text), Found: fs}
	}
	return rec
}
