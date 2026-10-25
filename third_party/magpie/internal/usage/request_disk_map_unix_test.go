//go:build unix

package usage

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync/atomic"
	"testing"
	"time"
	"unsafe"

	"github.com/yetone/magpie/internal/sessions"
)

// The chunks a start reads from the file are of the file's pages, not of
// the heap (#3493), and the pages are let go of once no chunk is of them.
func TestRequestDiskChunksMapped(t *testing.T) {
	pageHome(t)
	now := holdClock(t, time.Date(2026, 9, 30, 12, 0, 0, 0, time.Local))
	least := requestDiskLeast
	requestDiskLeast = 2
	t.Cleanup(func() { requestDiskLeast = least })
	var unmapped atomic.Int32
	diskUnmapped = func() { unmapped.Add(1) }
	t.Cleanup(func() { diskUnmapped = nil })
	dir := filepath.Join(sessions.CodexDir(), "sessions", "2026", "09", "30")
	for i := range 4 {
		writeCodexCall(t, filepath.Join(dir, fmt.Sprintf("rollout-2026-09-30T10-00-0%d-s%d.jsonl", i, i)), fmt.Sprintf("s%d", i), fmt.Sprintf("native-%d", i), `"cli"`, fmt.Sprintf("resp-%d", i), "m", "high", now.Add(-time.Duration(i+1)*time.Hour), 10*(i+1), i)
	}
	forget := func() {
		requestDiskSaves.Wait()
		requestCache.Lock()
		requestCache.chunks = map[string]*rowChunk{}
		requestCache.pages, requestCache.parts, requestCache.pairs, requestCache.heats = nil, nil, nil, nil
		requestCache.key = ""
		requestCache.Unlock()
		sessions.Reset()
	}
	first := queryPage(All, Filter{}, 0, 100, sessions.ReadCallSource)
	forget()
	equalPage(t, queryPage(All, Filter{}, 0, 100, sessions.ReadCallSource), first)

	requestCache.Lock()
	mapped := 0
	for _, c := range requestCache.chunks {
		if len(c.Archive) == 0 {
			continue
		}
		if c.mapped == nil {
			t.Errorf("%s: read from the file into the heap", filepath.Base(c.Source.Path))
			continue
		}
		at, start := uintptr(unsafe.Pointer(unsafe.SliceData(c.Archive))), uintptr(unsafe.Pointer(unsafe.SliceData(c.mapped.data)))
		if at < start || at+uintptr(len(c.Archive)) > start+uintptr(len(c.mapped.data)) {
			t.Errorf("%s: archive outside the mapping", filepath.Base(c.Source.Path))
		}
		mapped++
	}
	requestCache.Unlock()
	if mapped != 4 {
		t.Fatalf("%d chunks of the file's pages, want 4", mapped)
	}

	was := unmapped.Load()
	forget()
	for deadline := time.Now().Add(30 * time.Second); unmapped.Load() == was; {
		if time.Now().After(deadline) {
			t.Fatal("the mapping outlived every chunk of it")
		}
		runtime.GC()
		runtime.Gosched()
	}
}

// A file cut short under its mapping by another program is a read that
// failed, not a crash.
func TestRequestDiskMappingCutShort(t *testing.T) {
	path := filepath.Join(t.TempDir(), "requests-v1.bin")
	if err := os.WriteFile(path, make([]byte, 1<<20), 0o600); err != nil {
		t.Fatal(err)
	}
	m, ok := mapDiskFile(path)
	if !ok {
		t.Fatal("not mapped")
	}
	var last byte
	if !readMapped(m, func() { last = m.data[len(m.data)-1] }) || last != 0 {
		t.Fatal("a whole file's read failed")
	}
	if err := os.Truncate(path, 0); err != nil {
		t.Fatal(err)
	}
	if readMapped(m, func() { last = m.data[len(m.data)-1] }) {
		t.Fatal("a read past the cut passed")
	}
	runtime.KeepAlive(m)
}
