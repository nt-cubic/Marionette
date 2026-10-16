package sessions

import (
	"path/filepath"
	"sort"
	"testing"
	"time"
)

// #1951 (Elenoxe): Pi's and omp's calls on a provider of their own reach
// the usage ledger. The sessions are Pi 1.1.0's and omp 18.9.1's, written
// in a sandbox against a relay of their own (testdata/pi-calls,
// testdata/omp-calls; the system prompt's line left out, the sandbox's
// folder written /work/relay): two calls, a fork of that session with one
// more, and a session whose two calls failed (Connection error.) before it
// went on through magpie's entry, those lines as the owner's own Pi wrote
// them, the reply's text left out. The calls through magpie are the
// gateway's to count; a failed call is none; the fork's copy of its
// parent's calls are the parent's.
func TestPiCalls(t *testing.T) {
	claude, _ := setup(t)
	dir := filepath.Dir(claude)
	copyTree(t, "testdata/pi-calls", filepath.Join(dir, "pi"))
	copyTree(t, "testdata/omp-calls/agent", filepath.Join(dir, "home", ".omp", "agent"))
	const parent, fork = "01a1343b-2288-73d4-b1b4-6fb13abf0069", "01a1343b-9a0f-7603-b371-4124126b8b05"
	by := map[string][]Call{}
	for _, c := range Calls(time.Time{}) {
		by[c.Agent] = append(by[c.Agent], c)
		if c.Upstream == "magpie" || c.Upstream == "dead" {
			t.Errorf("a call through magpie or one that failed: %+v", c)
		}
	}
	want := []struct {
		session string
		at      string
		input   int
		millis  int64
	}{
		{parent, "2026-10-13T07:47:34.522Z", 201, 628},
		{parent, "2026-10-13T07:47:36.296Z", 202, 28},
		{fork, "2026-10-13T07:48:04.064Z", 203, 74},
	}
	got := by["pi"]
	if len(got) != len(want) {
		t.Fatalf("pi: %d calls, want %d: %+v", len(got), len(want), got)
	}
	sort.Slice(got, func(i, j int) bool { return got[i].Time.Before(got[j].Time) })
	for i, w := range want {
		c := got[i]
		at, _ := time.Parse(time.RFC3339Nano, w.at)
		if c.Session != w.session || !c.Time.Equal(at) || c.Model != "deepseek-v4" || c.Upstream != "relay" || c.Cwd != "/work/relay" ||
			c.Tokens != (Tokens{Input: w.input, Output: 30, CacheRead: 1000}) || c.Millis != w.millis || c.Msg == "" {
			t.Errorf("pi call %d: %+v", i, c)
		}
	}
	// the fork's file alone holds its own call, whichever file Calls reads
	// first
	for _, f := range piFiles() {
		if f.key == "pi:"+fork {
			if st, ok := readPiCalls(f); !ok || len(st.Calls) != 1 {
				t.Errorf("the fork's own calls: %+v", st.Calls)
			}
		}
	}
	o := by["omp"]
	if len(o) != 1 || o[0].Session != "01a1343c-ec54-759d-9aba-563bba5be0cf" || o[0].Upstream != "relay" ||
		o[0].Tokens != (Tokens{Input: 204, Output: 30, CacheRead: 1000}) || o[0].Millis != 21 || o[0].TTFT != 20 {
		t.Fatalf("omp: %+v", o)
	}
	// and a call holds what the session's summary holds for it: the
	// Sessions page and the Usage page tell the same tokens
	sum := map[string]Tokens{}
	for _, c := range got {
		s := sum[c.Session]
		s.add(c.Tokens)
		sum[c.Session] = s
	}
	for _, s := range List(0) {
		if s.Agent == "pi" && (s.ID == parent || s.ID == fork) && s.Tokens != sum[s.ID] {
			t.Errorf("session %s: %+v, its calls %+v", s.ID, s.Tokens, sum[s.ID])
		}
	}
}

// What a Pi session counts beside its replies is a call too: a tool's
// model work, a "usage" entry's call (a cache warm) on the provider it
// names; a compaction's on the model in use, here magpie's, is the
// gateway's (testdata/pi).
func TestPiCallsBesideReplies(t *testing.T) {
	_, _ = setupMore(t)
	got := map[string]Tokens{}
	for _, c := range Calls(time.Time{}) {
		if c.Agent == "pi" && c.Session == "0199aaaa-1111-7222-8333-444455556666" {
			got[c.Msg[:len("pi:e0000005")]] = c.Tokens
		}
	}
	want := map[string]Tokens{
		"pi:e0000005": {Input: 100, Output: 50, CacheRead: 1000, CacheWrite: 200},
		"pi:e0000006": {Input: 10, Output: 5},
		"pi:e0000008": {CacheRead: 300},
	}
	if len(got) != len(want) {
		t.Fatalf("calls %+v, want %+v", got, want)
	}
	for k, w := range want {
		if got[k] != w {
			t.Errorf("%s: %+v, want %+v", k, got[k], w)
		}
	}
}
