package redact_test

import (
	"context"
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
// Each local model (pii.Models) is scored on its own. What it said is
// recorded in testdata (evalModels), so the test runs the whole of
// "both" without the model or the network, and fails when a value that
// came out right comes out wrong. A value known to come out wrong with a
// model says so (evalValue.Known for Privacy Filter, GuardKnown for
// AstrLink Guard, an issue), and fails once it comes out right, to have
// it taken off.
//
// To ask the real models again, after a case is added or a new model,
// point MAGPIE_EVAL_CACHE at the cache folder they were downloaded into
// (the folder that holds magpie/privacy-filter, magpie/astrlink-guard);
// each one there is asked:
//
//	MAGPIE_EVAL_CACHE=$HOME/.cache go test -run TestMaskEval -v ./internal/redact
//
// which writes the recording again and logs the table.
func TestMaskEval(t *testing.T) {
	asked := 0
	for _, em := range evalModels {
		t.Run(em.id, func(t *testing.T) {
			rec := readRecording(t, em.path)
			if dir := os.Getenv("MAGPIE_EVAL_CACHE"); dir != "" {
				if r, ok := askModel(t, dir, pii.Of(em.id)); ok {
					rec = r
					writeRecording(t, em.path, rec)
					asked++
				}
			}
			evalModel(t, rec, em.known, pii.Of(em.id).MinScore())
		})
	}
	if os.Getenv("MAGPIE_EVAL_CACHE") != "" && asked == 0 {
		t.Fatal("no model is in MAGPIE_EVAL_CACHE")
	}
}

// evalModels are the local models, each with its recording and the
// values known to come out wrong with it.
var evalModels = []struct {
	id, path string
	known    func(evalValue) string
}{
	{pii.PrivacyFilter, "testdata/eval-model.json", func(v evalValue) string { return v.Known }},
	{pii.AstrLinkGuard, "testdata/eval-astrlink.json", func(v evalValue) string { return v.GuardKnown }},
}

// evalModel scores the recording rec, keeping what the model was at
// least min sure of, as masking does at that threshold.
func evalModel(t *testing.T, rec map[string]recorded, known func(evalValue) string, min float64) {
	var stale []string
	for _, c := range evalCases {
		if r, ok := rec[c.ID]; !ok || r.Text != textHash(c.Text) {
			stale = append(stale, c.ID)
		}
	}
	if len(stale) > 0 {
		t.Fatalf("the model's recording is not of cases %v; record it again with MAGPIE_EVAL_CACHE (see TestMaskEval)", stale)
	}
	kept := map[string]recorded{}
	for id, r := range rec {
		fs := []redact.Found{}
		for _, f := range r.Found {
			if f.Score >= min {
				fs = append(fs, f)
			}
		}
		kept[id] = recorded{Text: r.Text, Found: fs}
	}
	rec = kept
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
			checkValue(t, c, v, known(v), true, &tally, gone(rules, v.V), modelFound(c.Text, v.V, rec[c.ID].Found), gone(both, v.V))
		}
		for _, v := range c.Keep {
			checkValue(t, c, v, known(v), false, &tally, gone(rules, v.V), modelFound(c.Text, v.V, rec[c.ID].Found), gone(both, v.V))
		}
	}
	t.Log("\n" + tally.table())
}

// checkValue counts one value in each way, and holds "both" to what the
// case says of it.
func checkValue(t *testing.T, c evalCase, v evalValue, known string, mask bool, tally *evalTally, rules, model, both bool) {
	t.Helper()
	tally.add(v.Kind, mask, rules, model, both)
	if rules != mask || model != mask || both != mask {
		tally.wrong = append(tally.wrong, fmt.Sprintf("%-18s %-6s %-7s %-5s %-5s %-5s %q",
			c.ID, map[bool]string{true: "mask", false: "keep"}[mask], v.Kind, mark(rules == mask), mark(model == mask), mark(both == mask), v.V))
	}
	if known != "" {
		if both == mask {
			t.Errorf("%s: %q now comes out right; take its known issue (%s) off", c.ID, v.V, known)
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

func readRecording(t *testing.T, recordingPath string) map[string]recorded {
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

func writeRecording(t *testing.T, recordingPath string, rec map[string]recorded) {
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

// askModel runs every case's text through the real model m, downloaded
// into the cache folder dir, one text at a time, as a request's text
// that has the model to itself is; false if m isn't there.
func askModel(t *testing.T, dir string, m pii.Model) (map[string]recorded, bool) {
	t.Setenv("XDG_CACHE_HOME", dir)
	if !m.Installed() {
		t.Logf("%s isn't in %s; its recording is kept", m.ID(), filepath.Join(dir, "magpie"))
		return nil, false
	}
	defer m.Stop()
	rec := map[string]recorded{}
	for _, c := range evalCases {
		got, err := m.Find(context.Background(), []string{c.Text})
		if err != nil {
			t.Fatalf("%s: %v", c.ID, err)
		}
		fs := got[0]
		if fs == nil {
			fs = []redact.Found{}
		}
		rec[c.ID] = recorded{Text: textHash(c.Text), Found: fs}
	}
	return rec, true
}
