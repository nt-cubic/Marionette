package usage

import (
	"fmt"
	"reflect"
	"testing"
	"time"
	"unsafe"
)

// A chunk's rows keep the strings they share once, in Shapes, and only
// their own ids in the row: the page holds every row of the period at once,
// and 140 of a row's 328 bytes were string ids most rows of a chunk repeat.
// Packed and read back, every row is the row it was.
func TestRowsOfOneSessionShareAShape(t *testing.T) {
	if n := unsafe.Sizeof(packedRow{}); n > 216 {
		t.Fatalf("a packed row is %d bytes, want 216 at most", n)
	}
	at := time.Date(2026, 10, 21, 12, 0, 0, 0, time.Local)
	c := &rowChunk{}
	var want []Row
	for i := range 200 {
		r := Row{Record: Record{Time: at.Add(time.Duration(i) * time.Second), Agent: "claude", Provider: "anthropic", Model: "claude-opus-5-5",
			Session: "s1", Endpoint: "/v1/messages", Input: i, Output: 2 * i, Status: 200,
			RequestID: fmt.Sprint("req", i), ResponseID: fmt.Sprint("resp", i), Archive: fmt.Sprint("a", i)}, Source: "log"}
		if i%7 == 0 {
			r.Error, r.Status = fmt.Sprint("overloaded ", i), 529
		}
		if i == 150 {
			r.Model = "claude-sonnet-5-5"
		}
		c.add(r, fmt.Sprint("msg", i), int64(i), r.Error != "")
		want = append(want, c.row(i))
	}
	if len(c.Shapes) != 2 {
		t.Fatalf("200 rows of two models kept %d shapes, want 2", len(c.Shapes))
	}
	c.dict, c.shapeIDs = nil, nil
	packed := c.pack()
	if len(packed.Archive) == 0 || packed.Shapes != nil {
		t.Fatalf("not packed: %d archive bytes, %d shapes", len(packed.Archive), len(packed.Shapes))
	}
	back := packed.unpack()
	if back == nil || len(back.Rows) != len(want) || len(back.Shapes) != 2 {
		t.Fatalf("read back: %+v", back)
	}
	for i, w := range want {
		if got := back.row(i); !reflect.DeepEqual(got, w) {
			t.Fatalf("row %d read back as\n%+v\nwant\n%+v", i, got, w)
		}
		if msg := back.Strings[back.text(&back.Rows[i], rowMsg)]; msg != fmt.Sprint("msg", i) {
			t.Fatalf("row %d's message id read back as %q", i, msg)
		}
	}
}
