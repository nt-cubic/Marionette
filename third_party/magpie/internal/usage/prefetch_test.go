package usage

import (
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/sessions"
)

// The sources a query parses are read several at a time, each handed back
// as its own, in any order asked; never more than a window of them waits to
// be taken, and one not wanted is read when it is asked for.
func TestPrefetchSources(t *testing.T) {
	var sources []sessions.CallSource
	for i := range 200 {
		sources = append(sources, sessions.CallSource{Path: fmt.Sprint(i)})
	}
	var mu sync.Mutex
	reads := map[string]int{}
	var busy, most atomic.Int32
	readSource := func(s sessions.CallSource) []sessions.Call {
		n := busy.Add(1)
		for {
			m := most.Load()
			if n <= m || most.CompareAndSwap(m, n) {
				break
			}
		}
		time.Sleep(time.Millisecond)
		busy.Add(-1)
		mu.Lock()
		reads[s.Path]++
		mu.Unlock()
		return []sessions.Call{{Model: s.Path}}
	}
	want := func(s sessions.CallSource) bool { return s.Path != "7" }
	read := prefetchSources(sources, want, readSource)
	for i := range sources {
		if i == 50 {
			// nothing is taken for a while: the readers stop at the window
			time.Sleep(50 * time.Millisecond)
			mu.Lock()
			ahead := len(reads)
			mu.Unlock()
			if ahead > 50+32+8 {
				t.Fatalf("%d read with 50 taken", ahead)
			}
		}
		cs := read(i)
		if len(cs) != 1 || cs[0].Model != sources[i].Path {
			t.Fatalf("source %d: %+v", i, cs)
		}
	}
	if most.Load() < 2 {
		t.Fatalf("at most %d read at once", most.Load())
	}
	for _, s := range sources {
		if reads[s.Path] != 1 {
			t.Fatalf("%s read %d times", s.Path, reads[s.Path])
		}
	}

	// one or none wanted: read as asked, nothing ahead
	reads = map[string]int{}
	read = prefetchSources(sources[:3], func(s sessions.CallSource) bool { return s.Path == "1" }, readSource)
	if len(reads) != 0 {
		t.Fatalf("read ahead: %v", reads)
	}
	if cs := read(2); cs[0].Model != "2" {
		t.Fatalf("%+v", cs)
	}
}
