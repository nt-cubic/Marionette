package sessions

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// growSessions adds a line to each of moreSessions' files, so each has to
// be read again.
func growSessions(t *testing.T, claude string, n int) {
	t.Helper()
	dir := filepath.Join(claude, "projects", "-work-many")
	for i := range n {
		p := filepath.Join(dir, fmt.Sprintf("33333333-2222-3333-4444-%012d.jsonl", i))
		f, err := os.OpenFile(p, os.O_APPEND|os.O_WRONLY, 0)
		if err != nil {
			t.Fatal(err)
		}
		f.WriteString(ccMsg(fmt.Sprintf("n%d", i), "2026-09-21T11:00:00.000Z", 100, 10))
		f.Close()
	}
}

// A page asked for while the start's warm-up reads the changed files is
// answered with the parses kept of them, not held until the warm-up has
// read them all (#2776: the warm-up held the page for its whole read).
func TestReadNotHeldByAnotherRead(t *testing.T) {
	inZone(t, 0)
	claude, _ := setup(t)
	moreSessions(t, claude, 12)
	if n := len(List(0)); n < 12 {
		t.Fatalf("listed %d sessions", n)
	}
	growSessions(t, claude, 12)

	hold, started := make(chan struct{}), make(chan struct{})
	var once sync.Once
	var parsed atomic.Int32
	parseHook = func() {
		parsed.Add(1)
		once.Do(func() { close(started) })
		<-hold
	}
	release := sync.OnceFunc(func() { close(hold) })
	t.Cleanup(func() { release(); parseHook = nil })

	warm := make(chan struct{})
	go func() { defer close(warm); statsAt(0, statsNow) }()
	<-started

	listed := make(chan []Session, 1)
	go func() { listed <- List(0) }()
	select {
	case got := <-listed:
		if len(got) < 12 {
			t.Fatalf("listed %d sessions while another read ran", len(got))
		}
	case <-time.After(10 * time.Second):
		t.Fatal("List waited for another read of the same files to end")
	}
	managed := make(chan ManageView, 1)
	go func() {
		managed <- Manage(map[string]bool{"claude": true}, func([]AgentCount, map[string]map[string]bool) string { return "claude" })
	}()
	select {
	case v := <-managed:
		if len(v.Sessions) < 12 {
			t.Fatalf("managed %d sessions while another read ran", len(v.Sessions))
		}
	case <-time.After(10 * time.Second):
		t.Fatal("Manage waited for another read of the same files to end")
	}
	release()
	<-warm
	if n := parsed.Load(); n != 12 {
		t.Fatalf("the 12 changed files were read %d times", n)
	}
	// and what the warm-up read is in, for the next page
	parsed.Store(0)
	List(0)
	if n := parsed.Load(); n != 0 {
		t.Fatalf("read %d files again after the warm-up had read them", n)
	}
}

// A file never read before has no parse to serve: a page waits for the
// read already under way, rather than reading it a second time.
func TestReadWaitsForAFirstReadUnderWay(t *testing.T) {
	inZone(t, 0)
	claude, _ := setup(t)
	moreSessions(t, claude, 12)

	hold, started := make(chan struct{}), make(chan struct{})
	var once sync.Once
	var parsed atomic.Int32
	parseHook = func() {
		parsed.Add(1)
		once.Do(func() { close(started) })
		<-hold
	}
	release := sync.OnceFunc(func() { close(hold) })
	t.Cleanup(func() { release(); parseHook = nil })

	warm := make(chan struct{})
	go func() { defer close(warm); statsAt(0, statsNow) }()
	<-started
	before := parsed.Load()

	listed := make(chan []Session, 1)
	go func() { listed <- List(0) }()
	select {
	case <-listed:
		t.Fatal("List answered without the files being read")
	case <-time.After(200 * time.Millisecond):
	}
	release()
	var got []Session
	select {
	case got = <-listed:
	case <-time.After(30 * time.Second):
		t.Fatal("List never answered")
	}
	<-warm
	if len(got) < 12 {
		t.Fatalf("listed %d sessions", len(got))
	}
	total := before
	mu.Lock()
	n := len(cache)
	mu.Unlock()
	if p := parsed.Load(); p != int32(n) {
		t.Fatalf("%d files read %d times (%d before the page asked)", n, p, total)
	}
}

// The stores two reads share stay open until the last of them is done: a
// read that ends first must not close what the other still reads.
func TestStoresLiveAsLongAsTheirLastUser(t *testing.T) {
	path := filepath.Join(t.TempDir(), "store.db")
	if err := os.WriteFile(path, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	first := useDBs()
	db := openDB(path)
	second := useDBs()
	if openDB(path) != db {
		mu.Unlock()
		t.Fatal("a second read opened its own handle")
	}
	second()
	mu.Unlock()
	if db == nil {
		t.Fatal("not opened")
	}
	if err := db.Ping(); err != nil {
		t.Fatalf("closed while a read still used it: %v", err)
	}
	mu.Lock()
	first()
	open := dbs[path] != nil
	mu.Unlock()
	if open || db.Ping() == nil {
		t.Fatal("left open after the last read")
	}
}
