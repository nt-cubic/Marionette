// Package maskrec keeps the record of what masking swapped for
// placeholders before a request went to a vendor, for Settings → Privacy
// to show: which kinds, found by which finder, going to whom.
//
// A value itself is never written down. A record holds each value's
// placeholder, its kind, its finder, and a hint (its first character or a
// key's prefix, and its length). The value behind a placeholder is known
// only to the masker's memory while magpie runs (gwmw.Teller.Value), and
// the text around it as the vendor saw it is kept in memory only, for the
// latest records.
//
// A day is a file of its own, one request a line, under
// ~/.config/magpie/masking, kept keepDays days and keepBytes in all.
package maskrec

import (
	"bufio"
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/appdir"
)

const (
	keepDays  = 30
	keepBytes = 100 << 20
	// keepSnips is how many of the latest records memory keeps, with the
	// text around their values
	keepSnips = 400
	dayForm   = "2006-01-02"
)

// Value is one value a request had masked, as often as it had it.
type Value struct {
	Placeholder string `json:"p"`
	Kind        string `json:"k"`
	By          string `json:"b"`           // the finder: rule, yours, word, model or known
	Head        string `json:"h,omitempty"` // its first character, or a key's prefix
	Len         int    `json:"l"`           // its length in characters
	N           int    `json:"n"`           // the places the request had it
	// Action: "" masked, "warn" sent as written (the user asked to be
	// told only), "block" masked and the request held back
	Action string `json:"a,omitempty"`
}

// Record is one request's masking.
type Record struct {
	Key      int64     `json:"key"` // unique, later records larger
	Time     time.Time `json:"t"`
	Route    int64     `json:"route,omitempty"` // its route in the trace, where it has one
	What     string    `json:"what"`            // chat, search, image, video, embeddings, audio, count
	Agent    string    `json:"agent,omitempty"`
	Session  string    `json:"session,omitempty"`
	Provider string    `json:"provider,omitempty"`
	Model    string    `json:"model,omitempty"`
	N        int       `json:"n"` // values swapped, each place counted
	Values   []Value   `json:"values,omitempty"`
	// Blocked: the request was held back, sent to no one, for a value
	// of a kind the user set to block
	Blocked bool `json:"blocked,omitempty"`
	// Snips: by placeholder, the text around its first place as the
	// vendor saw it; in memory only
	Snips map[string]string `json:"snips,omitempty"`
}

// Dir is where the days are kept.
var Dir = func() string { return filepath.Join(appdir.Config(), "masking") }

// clock is what day it is; a variable for the tests.
var clock = time.Now

var store struct {
	mu     sync.Mutex
	last   int64
	pruned time.Time
	days   map[string]*dayAgg // a day's file → what it holds, as of its size
	snips  []Record           // the latest records, with their snips
}

// inTest says Add writes nothing: in a test binary testenv isolated, a
// write still going as a test ends would put masking/ back into its
// temporary HOME (see the gateway's keepsRoutes).
func inTest() bool { return os.Getenv("MAGPIE_TEST_SANDBOX") != "" && !ownTest }

// ownTest is set by this package's tests, which read what they write.
var ownTest = false

// Add records r, its Values made from hits as they came: one for each
// placeholder, counted. It returns r as recorded. Errors are swallowed:
// keeping the record must never break a request.
func Add(r Record) Record {
	if len(r.Values) == 0 && r.N == 0 {
		return r
	}
	r.Values = merged(r.Values)
	if r.Time.IsZero() {
		r.Time = clock()
	}
	store.mu.Lock()
	defer store.mu.Unlock()
	r.Key = r.Time.UnixNano()
	if r.Key <= store.last {
		r.Key = store.last + 1
	}
	store.last = r.Key
	store.snips = append(store.snips, r)
	if len(store.snips) > keepSnips {
		store.snips = append([]Record(nil), store.snips[len(store.snips)-keepSnips:]...)
	}
	if inTest() {
		return r
	}
	disk := r
	disk.Snips = nil
	b, err := json.Marshal(disk)
	if err != nil {
		return r
	}
	dir := Dir()
	if os.MkdirAll(dir, 0o700) != nil {
		return r
	}
	path := filepath.Join(dir, r.Time.Local().Format(dayForm)+".jsonl")
	f, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return r
	}
	before, _ := f.Seek(0, 2)
	_, err = f.Write(append(b, '\n'))
	f.Close()
	// a day already read takes the record as it is written, rather than
	// being read again
	if a := store.days[path]; err == nil && a != nil && a.size == before {
		a.add(disk)
		a.size += int64(len(b) + 1)
	}
	if now := clock(); now.Sub(store.pruned) > time.Hour {
		store.pruned = now
		prune(dir, now)
	}
	return r
}

// merged is vs with one Value for each placeholder, its N the sum, in the
// order they first came. A value found by a finder and later only known
// (masked before) keeps the finder that found it.
func merged(vs []Value) []Value {
	at := map[string]int{}
	var out []Value
	for _, v := range vs {
		if v.N == 0 {
			v.N = 1
		}
		if i, ok := at[v.Placeholder]; ok {
			out[i].N += v.N
			if out[i].By == "known" && v.By != "known" {
				out[i].By = v.By
			}
			if v.Action == "block" {
				out[i].Action = v.Action
			}
			continue
		}
		at[v.Placeholder] = len(out)
		out = append(out, v)
	}
	return out
}

type dayFile struct{ day, path string }

// files are the days kept, oldest first.
func files(dir string) []dayFile {
	es, _ := os.ReadDir(dir)
	var out []dayFile
	for _, e := range es {
		n := e.Name()
		if day, ok := strings.CutSuffix(n, ".jsonl"); ok {
			if _, err := time.Parse(dayForm, day); err == nil {
				out = append(out, dayFile{day, filepath.Join(dir, n)})
			}
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].day < out[j].day })
	return out
}

// prune drops the days older than keepDays, then the oldest until what is
// left fits keepBytes.
func prune(dir string, now time.Time) {
	oldest := now.Local().AddDate(0, 0, -keepDays).Format(dayForm)
	fs := files(dir)
	var total int64
	sizes := make([]int64, len(fs))
	for i, d := range fs {
		if fi, err := os.Stat(d.path); err == nil {
			sizes[i] = fi.Size()
			total += sizes[i]
		}
	}
	for i, d := range fs {
		if d.day >= oldest && total <= keepBytes {
			break
		}
		if os.Remove(d.path) == nil {
			total -= sizes[i]
			delete(store.days, d.path)
		}
	}
}

// read calls each with the records of path, in the order written; a line
// that doesn't parse, one half-written by another magpie, is skipped.
func read(path string, each func(Record)) int64 {
	f, err := os.Open(path)
	if err != nil {
		return 0
	}
	defer f.Close()
	var size int64
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 64<<10), 16<<20)
	for sc.Scan() {
		line := sc.Bytes()
		size += int64(len(line) + 1)
		if len(bytes.TrimSpace(line)) == 0 {
			continue
		}
		var r Record
		if json.Unmarshal(line, &r) == nil {
			each(r)
		}
	}
	return size
}

// withSnips is r with the snips memory has for it.
func withSnips(r Record) Record {
	i := sort.Search(len(store.snips), func(i int) bool { return store.snips[i].Key >= r.Key })
	if i < len(store.snips) && store.snips[i].Key == r.Key {
		r.Snips = store.snips[i].Snips
	}
	return r
}

// Filter narrows List: by kind, by finder, by agent or provider; "" is
// any.
type Filter struct {
	Kind, By, Agent, Provider string
}

func (f Filter) takes(r Record) bool {
	if f.Agent != "" && r.Agent != f.Agent || f.Provider != "" && r.Provider != f.Provider {
		return false
	}
	if f.Kind == "" && f.By == "" {
		return true
	}
	for _, v := range r.Values {
		if (f.Kind == "" || v.Kind == f.Kind) && (f.By == "" || v.By == f.By) {
			return true
		}
	}
	return false
}

// List is up to limit records before key (0: the latest), newest first,
// that f takes, and whether there are more.
func List(before int64, limit int, f Filter) ([]Record, bool) {
	store.mu.Lock()
	defer store.mu.Unlock()
	if limit <= 0 {
		limit = 50
	}
	var out []Record
	fs := files(Dir())
	for i := len(fs) - 1; i >= 0; i-- {
		if before > 0 && time.Unix(0, before).Local().Format(dayForm) < fs[i].day {
			continue
		}
		var day []Record
		read(fs[i].path, func(r Record) {
			if (before == 0 || r.Key < before) && f.takes(r) {
				day = append(day, r)
			}
		})
		for j := len(day) - 1; j >= 0; j-- {
			if len(out) == limit {
				return out, true
			}
			out = append(out, withSnips(day[j]))
		}
	}
	if inTest() {
		// nothing on disk: the records in memory
		for j := len(store.snips) - 1; j >= 0; j-- {
			r := store.snips[j]
			if (before == 0 || r.Key < before) && f.takes(r) {
				if len(out) == limit {
					return out, true
				}
				out = append(out, r)
			}
		}
	}
	return out, false
}

// Get is the record of key, with its snips while memory has them.
func Get(key int64) (Record, bool) {
	store.mu.Lock()
	defer store.mu.Unlock()
	if i := sort.Search(len(store.snips), func(i int) bool { return store.snips[i].Key >= key }); i < len(store.snips) && store.snips[i].Key == key {
		return store.snips[i], true
	}
	day := time.Unix(0, key).Local().Format(dayForm)
	var got Record
	ok := false
	read(filepath.Join(Dir(), day+".jsonl"), func(r Record) {
		if r.Key == key {
			got, ok = r, true
		}
	})
	return got, ok
}

// OfRoute is the record of the trace's route id, while memory has it.
func OfRoute(id int64) (Record, bool) {
	store.mu.Lock()
	defer store.mu.Unlock()
	for j := len(store.snips) - 1; j >= 0; j-- {
		if store.snips[j].Route == id {
			return store.snips[j], true
		}
	}
	return Record{}, false
}
