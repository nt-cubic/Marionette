package usage

import (
	"fmt"
	"math"
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
	if n := unsafe.Sizeof(packedRow{}); n > 152 {
		t.Fatalf("a packed row is %d bytes, want 152 at most", n)
	}
	at := time.Date(2026, 10, 21, 12, 0, 0, 0, time.Local)
	c := &rowChunk{}
	var want []Row
	for i := range 200 {
		r := Row{Record: Record{Time: at.Add(time.Duration(i) * time.Second), Agent: "claude", Provider: "anthropic", Model: "claude-opus-5-5",
			Session: "s1", Endpoint: "/v1/messages", Input: i, Output: 2 * i, Own: int64(3 * i), Masking: int64(i), Status: 200,
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

// A row keeps its times in ms in 32 bits: one longer than that, which no
// call takes, reads back as the longest it can keep, not wrapped around.
func TestRowTimesPastWhatARowKeeps(t *testing.T) {
	long := int64(30 * 24 * time.Hour / time.Millisecond)
	c := &rowChunk{}
	c.add(Row{Record: Record{Time: time.Date(2026, 10, 21, 12, 0, 0, 0, time.UTC), Millis: long, Queued: -long, TTFT: 1500}}, "", 0, false)
	got := c.row(0)
	if got.Millis != math.MaxInt32 || got.Queued != math.MinInt32 || got.TTFT != 1500 {
		t.Fatalf("read back Millis %d, Queued %d, TTFT %d; want %d, %d, 1500", got.Millis, got.Queued, got.TTFT, int64(math.MaxInt32), int64(math.MinInt32))
	}
}

// A row's tokens are kept in 32 bits and its time as ns beside its zone:
// a count past 2^31 is kept as the most an int32 holds, not wrapped, and
// packed and read back, the time is the instant it was in the zone it
// was, and a zero time stays zero.
func TestRowTokensAndZoneKept(t *testing.T) {
	zone := time.FixedZone("IST", 5*3600+1800)
	at := time.Date(2026, 10, 21, 12, 0, 0, 123, zone)
	c := &rowChunk{}
	c.add(Row{Record: Record{Time: at, Input: 3_000_000_000, Output: -3_000_000_000, CacheRead: 7}}, "", 0, false)
	c.add(Row{Record: Record{Input: 1}}, "", 1, false)
	for _, c := range []*rowChunk{c, c.pack().unpack()} {
		got := c.row(0)
		if got.Input != math.MaxInt32 || got.Output != math.MinInt32 || got.CacheRead != 7 {
			t.Fatalf("read back Input %d, Output %d, CacheRead %d; want %d, %d, 7", got.Input, got.Output, got.CacheRead, math.MaxInt32, math.MinInt32)
		}
		if !got.Time.Equal(at) || got.Time.Location() != zone || got.Time.Format(time.RFC3339Nano) != at.Format(time.RFC3339Nano) {
			t.Fatalf("read back time %v in %v, want %v", got.Time, got.Time.Location(), at)
		}
		if z := c.row(1).Time; !z.IsZero() {
			t.Fatalf("a zero time read back as %v", z)
		}
	}
}
