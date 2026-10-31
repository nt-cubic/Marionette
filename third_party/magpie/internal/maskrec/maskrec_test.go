package maskrec

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func sandbox(t *testing.T, now time.Time) string {
	t.Helper()
	dir := t.TempDir()
	oldDir, oldClock, oldOwn := Dir, clock, ownTest
	Dir, clock, ownTest = func() string { return dir }, func() time.Time { return now }, true
	store.mu.Lock()
	store.days, store.snips, store.sent, store.sentBytes, store.last, store.pruned = nil, nil, nil, 0, 0, time.Time{}
	store.first, store.firstRead = nil, false
	store.mu.Unlock()
	t.Cleanup(func() { Dir, clock, ownTest = oldDir, oldClock, oldOwn })
	return dir
}

// What is written to disk is each value's placeholder, kind, finder and
// hint: never the value, and never the text around it, which memory keeps.
func TestNoValueOnDisk(t *testing.T) {
	now := time.Date(2026, 10, 20, 15, 0, 0, 0, time.Local)
	dir := sandbox(t, now)
	const name, around = "张伟", "请联系{{PERSON_aaaabbbb}}老师确认合同"
	r := Add(Record{Time: now, Route: 7, What: "chat", Agent: "claude", Provider: "anthropic", Model: "sonnet", N: 3,
		Values: []Value{
			{Placeholder: "{{PERSON_aaaabbbb}}", Kind: "PERSON", By: "model", Head: "张", Len: 2},
			{Placeholder: "{{PERSON_aaaabbbb}}", Kind: "PERSON", By: "known", Head: "张", Len: 2},
			{Placeholder: "{{API_KEY_ccccdddd}}", Kind: "API_KEY", By: "rule", Head: "sk-", Len: 40},
		},
		Snips: map[string]string{"{{PERSON_aaaabbbb}}": around}})
	if len(r.Values) != 2 || r.Values[0].N != 2 || r.Values[0].By != "model" {
		t.Fatalf("values not merged by placeholder: %+v", r.Values)
	}
	b, err := os.ReadFile(filepath.Join(dir, "2026-10-20.jsonl"))
	if err != nil {
		t.Fatal(err)
	}
	for _, secret := range []string{name, "老师", "合同"} {
		if strings.Contains(string(b), secret) {
			t.Fatalf("disk has %q: %s", secret, b)
		}
	}
	if got, ok := Get(r.Key); !ok || got.Snips["{{PERSON_aaaabbbb}}"] != around {
		t.Fatalf("memory lost the snip: %+v", got)
	}
	if got, ok := OfRoute(7); !ok || got.Key != r.Key {
		t.Fatalf("OfRoute: %+v %v", got, ok)
	}
	// after a restart: the record, without its snips
	store.mu.Lock()
	store.snips = nil
	store.mu.Unlock()
	got, ok := Get(r.Key)
	if !ok || got.Snips != nil || len(got.Values) != 2 || got.Values[1].Head != "sk-" {
		t.Fatalf("after restart: %+v %v", got, ok)
	}
}

// The summary counts requests, places and different values, by day, kind,
// finder, agent and provider; a value found by the model and only known
// after is the model's.
func TestSum(t *testing.T) {
	now := time.Date(2026, 10, 20, 15, 0, 0, 0, time.Local)
	sandbox(t, now)
	p, k := "{{PERSON_aaaabbbb}}", "{{API_KEY_ccccdddd}}"
	Add(Record{Time: now.AddDate(0, 0, -2), Agent: "codex", Provider: "openai", N: 1, Values: []Value{{Placeholder: p, Kind: "PERSON", By: "model", Len: 2}}})
	_ = Sum(7) // read, then kept up as Add writes
	Add(Record{Time: now.Add(-time.Hour), Agent: "claude", Provider: "anthropic", N: 3, Values: []Value{{Placeholder: p, Kind: "PERSON", By: "known", Len: 2, N: 2}, {Placeholder: k, Kind: "API_KEY", By: "rule", Len: 40}}})
	Add(Record{Time: now, Agent: "claude", Provider: "anthropic", N: 1, Values: []Value{{Placeholder: k, Kind: "API_KEY", By: "rule", Len: 40}}})
	s := Sum(7)
	if s.Requests != 3 || s.N != 5 || s.Distinct != 2 || len(s.Days) != 7 {
		t.Fatalf("totals: %+v", s)
	}
	if s.Days[6].Requests != 2 || s.Days[6].Distinct != 2 || s.Days[4].Requests != 1 {
		t.Fatalf("days: %+v", s.Days)
	}
	if s.By["model"] != 1 || s.By["rule"] != 1 || s.By["known"] != 0 {
		t.Fatalf("by: %v", s.By)
	}
	if len(s.Kinds) != 2 || s.Kinds[0].Name != "PERSON" || s.Kinds[0].N != 3 || s.Kinds[0].Distinct != 1 {
		t.Fatalf("kinds: %+v", s.Kinds)
	}
	if len(s.Agents) != 2 || s.Agents[0].Name != "claude" || s.Agents[0].Distinct != 2 || s.Providers[0].Name != "anthropic" {
		t.Fatalf("agents %+v providers %+v", s.Agents, s.Providers)
	}
	if s.Values[0].Placeholder != p || s.Values[0].Requests != 2 || s.Values[0].By != "model" {
		t.Fatalf("values: %+v", s.Values)
	}
	if s.Hours[14] != 3 || s.Hours[15] != 2 {
		t.Fatalf("hours: %v", s.Hours)
	}
	// only one kind or finder
	if rs, more := List(0, 10, Filter{By: "model"}); len(rs) != 1 || more || rs[0].Agent != "codex" {
		t.Fatalf("filtered: %+v", rs)
	}
	// several kinds, split by commas
	if rs, _ := List(0, 10, Filter{Kind: "EMAIL,API_KEY"}); len(rs) != 2 || rs[0].Agent != "claude" {
		t.Fatalf("kinds: %+v", rs)
	}
	rs, more := List(0, 2, Filter{})
	if len(rs) != 2 || !more || !rs[0].Time.Equal(now) {
		t.Fatalf("page: %+v %v", rs, more)
	}
	if rest, more := List(rs[1].Key, 2, Filter{}); len(rest) != 1 || more || rest[0].Agent != "codex" {
		t.Fatalf("next page: %+v %v", rest, more)
	}
}

// Days older than keepDays go.
func TestPrune(t *testing.T) {
	now := time.Date(2026, 10, 20, 15, 0, 0, 0, time.Local)
	dir := sandbox(t, now)
	old := filepath.Join(dir, "2026-09-01.jsonl")
	os.MkdirAll(dir, 0o700)
	os.WriteFile(old, []byte("{}\n"), 0o600)
	Add(Record{Time: now, N: 1, Values: []Value{{Placeholder: "{{X_aaaaaaaa}}", Kind: "X", By: "rule"}}})
	if _, err := os.Stat(old); !os.IsNotExist(err) {
		t.Fatalf("old day kept: %v", err)
	}
}

// A value sent as written is counted as warned, not as one masked, and a
// request held back is counted, read from disk and kept up alike (#1977).
func TestSumWarnedAndBlocked(t *testing.T) {
	now := time.Date(2026, 10, 20, 15, 0, 0, 0, time.Local)
	sandbox(t, now)
	e, k := "{{EMAIL_aaaabbbb}}", "{{API_KEY_ccccdddd}}"
	Add(Record{Time: now.Add(-time.Hour), N: 1, Values: []Value{{Placeholder: e, Kind: "EMAIL", By: "rule", Len: 9, N: 2, Action: "warn"}, {Placeholder: k, Kind: "API_KEY", By: "rule", Len: 40}}})
	for _, fresh := range []bool{true, false} {
		if fresh {
			store.mu.Lock()
			store.days = nil
			store.mu.Unlock()
		}
		_ = Sum(7)
		Add(Record{Time: now, What: "blocked", Blocked: true, N: 1, Values: []Value{{Placeholder: k, Kind: "API_KEY", By: "rule", Len: 40, Action: "block"}}})
	}
	s := Sum(7)
	if s.Warned != 2 || s.Blocked != 2 || s.Distinct != 1 || s.N != 3 || len(s.Kinds) != 1 || s.Kinds[0].Name != "API_KEY" {
		t.Fatalf("summary: %+v", s)
	}
	if got, _ := List(0, 1, Filter{}); len(got) != 1 || !got[0].Blocked || got[0].Values[0].Action != "block" {
		t.Fatalf("record: %+v", got)
	}
	// only the requests held back
	if got, _ := List(0, 10, Filter{Blocked: true}); len(got) != 2 || !got[0].Blocked || !got[1].Blocked {
		t.Fatalf("held back: %+v", got)
	}
}

// A held record goes on disk with the provider and model done names, and
// the summary counts that provider, never the planned one. A long request
// held before a short one is written after it, and the list is still in
// key order, so the next page's before skips nothing.
func TestHoldNamesWhoHadIt(t *testing.T) {
	now := time.Date(2026, 10, 26, 12, 0, 0, 0, time.Local)
	sandbox(t, now)
	v := []Value{{Placeholder: "{{API_KEY_aaaabbbb}}", Kind: "API_KEY", By: "rule", Head: "sk-", Len: 40}}
	long, done := Hold(Record{Time: now, What: "chat", Provider: "first", Model: "m1", N: 1, Values: v})
	if got, ok := Get(long.Key); !ok || got.Provider != "first" {
		t.Fatalf("held record not in memory: %+v %v", got, ok)
	}
	short := Add(Record{Time: now, What: "chat", Provider: "other", N: 1, Values: v})
	done("second", "m2")
	done("third", "m3") // only the first call counts
	sum := Sum(1)
	names := []string{}
	for _, p := range sum.Providers {
		names = append(names, p.Name)
	}
	if strings.Join(names, ",") != "other,second" && strings.Join(names, ",") != "second,other" {
		t.Fatalf("summary counts %v", names)
	}
	store.mu.Lock()
	store.snips = nil // after a restart: what disk has
	store.mu.Unlock()
	got, ok := Get(long.Key)
	if !ok || got.Provider != "second" || got.Model != "m2" {
		t.Fatalf("on disk: %+v %v", got, ok)
	}
	page, more := List(0, 1, Filter{})
	if len(page) != 1 || page[0].Key != short.Key || !more {
		t.Fatalf("first page %+v %v", page, more)
	}
	page, _ = List(page[0].Key, 1, Filter{})
	if len(page) != 1 || page[0].Key != long.Key {
		t.Fatalf("second page %+v", page)
	}
}

// Clear deletes every day kept and what memory has of the records, at
// once rather than after 30 days (maxyou2090 on Discord: strict masking
// recorded a lot); a file beside the days that isn't one stays, and the
// next request is recorded again.
func TestClear(t *testing.T) {
	now := time.Date(2026, 10, 26, 15, 0, 0, 0, time.Local)
	dir := sandbox(t, now)
	for back := range 3 {
		at := now.AddDate(0, 0, -back)
		Add(Record{Time: at, What: "chat", Agent: "claude", N: 1, Sent: `{"k":"{{API_KEY_aaaabbbb}}"}`,
			Values: []Value{{Placeholder: "{{API_KEY_aaaabbbb}}", Kind: "API_KEY", By: "rule", Head: "sk-", Len: 40}},
			Snips:  map[string]string{"{{API_KEY_aaaabbbb}}": "key {{API_KEY_aaaabbbb}}"}})
	}
	os.WriteFile(filepath.Join(dir, "notes.txt"), []byte("mine"), 0o600)
	last, _ := List(0, 1, Filter{})
	if s := Sum(7); s.Requests != 3 || len(last) != 1 {
		t.Fatalf("before: %d requests, %d listed", s.Requests, len(last))
	}
	if err := Clear(); err != nil {
		t.Fatal(err)
	}
	es, _ := os.ReadDir(dir)
	if len(es) != 1 || es[0].Name() != "notes.txt" {
		t.Fatalf("left in %s: %v", dir, es)
	}
	if rs, more := List(0, 10, Filter{}); len(rs) != 0 || more {
		t.Fatalf("listed after clear: %+v", rs)
	}
	if s := Sum(30); s.Requests != 0 || s.N != 0 || len(s.Values) != 0 || s.Since != "" {
		t.Fatalf("summed after clear: %+v", s)
	}
	if _, ok := Get(last[0].Key); ok {
		t.Fatal("a cleared record is still got")
	}
	if _, ok := SentOf(last[0].Key); ok {
		t.Fatal("a cleared request's text is still in memory")
	}
	r := Add(Record{Time: now, What: "chat", N: 1, Values: []Value{{Placeholder: "{{EMAIL_cccc0000}}", Kind: "EMAIL", By: "rule"}}})
	if r.Key <= last[0].Key {
		t.Fatalf("a key after clear went back: %d <= %d", r.Key, last[0].Key)
	}
	if s := Sum(7); s.Requests != 1 {
		t.Fatalf("after clear and one more: %+v", s)
	}
}

// A day that can't be removed is named in the error and stays listed:
// the page doesn't say cleared while records are left.
func TestClearSaysWhatStays(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("a folder's write bit doesn't hold a file on Windows")
	}
	now := time.Date(2026, 10, 26, 15, 0, 0, 0, time.Local)
	dir := sandbox(t, now)
	Add(Record{Time: now, What: "chat", N: 1, Values: []Value{{Placeholder: "{{EMAIL_cccc0000}}", Kind: "EMAIL", By: "rule"}}})
	os.Chmod(dir, 0o500)
	t.Cleanup(func() { os.Chmod(dir, 0o700) })
	if probe := filepath.Join(dir, "probe"); os.WriteFile(probe, nil, 0o600) == nil {
		os.Remove(probe)
		t.Skip("running as a user a folder's mode doesn't hold back")
	}
	err := Clear()
	if err == nil || !strings.Contains(err.Error(), "2026-10-26.jsonl") {
		t.Fatalf("err %v", err)
	}
	if rs, _ := List(0, 10, Filter{}); len(rs) != 1 {
		t.Fatalf("the day left isn't listed: %+v", rs)
	}
}
