package usage

import (
	"fmt"
	"runtime"
	"testing"
	"time"
)

// A packed block keeps its compressed bytes and no more. s2.Encode's
// output is as large as the worst case, more than the rows' own bytes,
// and an archive cut from it (even clipped) held that whole allocation:
// several times the compressed bytes Bytes counts, for as long as the
// block is cached.
func TestPackedArchiveHoldsOnlyItsBytes(t *testing.T) {
	now := time.Now()
	block := func(k, n int) *rowChunk {
		c := &rowChunk{}
		for i := range n {
			r := Row{Record: Record{Time: now.Add(time.Duration(i) * time.Second), Agent: "claude", Provider: "anthropic", Model: "claude-opus-5-5", Input: 1200 + i%7, Output: 300, Session: fmt.Sprint("s", k, "-", i/50)}}
			c.add(r, fmt.Sprint("msg", i), int64(i), false)
		}
		return c
	}
	var base, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&base)
	packed := make([]*rowChunk, 100)
	archives := 0
	for k := range packed {
		packed[k] = block(k, 2000).pack()
		if len(packed[k].Archive) == 0 {
			t.Fatalf("block %d: not packed", k)
		}
		archives += len(packed[k].Archive)
	}
	runtime.GC()
	runtime.ReadMemStats(&after)
	// the unpacked rows went with the collection: what stays is the
	// archives, their chunks' headers and the scratch the pool keeps
	if held := int64(after.HeapAlloc) - int64(base.HeapAlloc); held > int64(archives)*3/2+1<<20 {
		t.Fatalf("100 packed blocks hold %d KB for %d KB of archives", held>>10, archives>>10)
	}
	for k, c := range packed {
		back := c.unpack()
		if back == nil || len(back.Rows) != 2000 || back.row(1999).Input != 1200+1999%7 || back.row(1999).Session != fmt.Sprint("s", k, "-", 1999/50) {
			t.Fatalf("block %d: not read back", k)
		}
	}
	// blocks packed one after another through the same scratch keep their
	// own bytes
	a, b := &rowChunk{}, &rowChunk{}
	for i := range 100 {
		a.add(Row{Record: Record{Time: now, Agent: "a", Model: fmt.Sprint("ma", i)}}, "", int64(i), false)
		b.add(Row{Record: Record{Time: now, Agent: "b", Model: fmt.Sprint("mb", i)}}, "", int64(i), false)
	}
	pa := a.pack()
	pb := b.pack()
	if ua, ub := pa.unpack(), pb.unpack(); ua.row(99).Model != "ma99" || ub.row(99).Model != "mb99" {
		t.Fatalf("read back %q and %q", ua.row(99).Model, ub.row(99).Model)
	}
}
