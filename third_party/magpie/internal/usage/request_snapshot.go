package usage

// Cache snapshots encode the fixed request fields without reflection. The
// encoding is private to this process: no on-disk format or migration. Time
// locations are retained separately so a round trip preserves displayed times.
//
// The blocks are compressed with S2, not flate: a query over the whole
// history decodes every block, and flate's decoder was most of the 3 s a
// filter over 750k calls took; S2 decodes at memory speed for a block a
// little larger.
import (
	"encoding/binary"
	"math"
	"slices"
	"sync"
	"time"
	"unsafe"

	"github.com/klauspost/compress/s2"
)

// rowDecodes are the buffers blocks are decoded into before their rows are
// read out of them; nothing keeps a reference to one after unpack.
var rowDecodes = sync.Pool{New: func() any { return new([]byte) }}

func (c *rowChunk) pack() *rowChunk {
	if c == nil || len(c.Rows) < 64 || len(c.Archive) != 0 {
		return c
	}
	data := make([]byte, 0, len(c.Rows)*64)
	put := func(v uint64) { data = binary.AppendUvarint(data, v) }
	put(uint64(len(c.Rows)))
	put(uint64(len(c.Strings)))
	for _, s := range c.Strings {
		put(uint64(len(s)))
		data = append(data, s...)
	}
	var locations []*time.Location
	locs := map[*time.Location]uint64{}
	for k := range c.Rows {
		p := &c.Rows[k]
		loc := p.Time.Location()
		id, ok := locs[loc]
		if !ok {
			id = uint64(len(locations))
			locs[loc] = id
			locations = append(locations, loc)
		}
		put(uint64(p.Time.Unix()))
		put(uint64(p.Time.Nanosecond()))
		put(id)
		for _, n := range p.Text {
			put(uint64(n))
		}
		for _, n := range p.Tokens {
			put(uint64(n))
		}
		for _, n := range []int64{p.Millis, p.TTFT, p.FirstText, p.Order, p.RouteID, p.Sent, p.Flow, p.Own, p.Retried, p.Queued, p.FirstReasoning} {
			put(uint64(n))
		}
		put(math.Float64bits(p.Cost))
		put(uint64(p.Status))
		put(uint64(p.Flags))
		put(uint64(p.Tries))
		if p.Flags&rowCredits != 0 {
			put(math.Float64bits(p.Credits))
		}
	}
	archive := s2.Encode(nil, data)
	if int64(len(archive)) >= c.Bytes {
		return c
	}
	next := *c
	next.Rows, next.Strings, next.dict = nil, nil, nil
	next.Archive, next.Locations = slices.Clip(archive), locations
	next.Bytes = int64(len(next.Archive)+len(locations)*8) + codexActorBytes(next.codexActors)
	return &next
}

func (c *rowChunk) unpack() *rowChunk { return c.unpackInto(nil) }

// unpackInto is unpack decoding the rows into rows when it is large enough:
// every field of a row is written, so a reused slice needs no clearing.
func (c *rowChunk) unpackInto(into []packedRow) *rowChunk {
	if c == nil || len(c.Archive) == 0 {
		return c
	}
	buf := rowDecodes.Get().(*[]byte)
	defer rowDecodes.Put(buf)
	data, err := s2.Decode((*buf)[:cap(*buf)], c.Archive)
	if err != nil {
		return nil
	}
	*buf = data
	valid := true
	take := func() uint64 {
		n, k := binary.Uvarint(data)
		if k <= 0 {
			valid = false
			return 0
		}
		data = data[k:]
		return n
	}
	rows, strs := take(), take()
	if rows > uint64(len(data)) || strs > uint64(len(data)) {
		return nil
	}
	next := *c
	next.Archive, next.Locations = nil, nil
	if uint64(cap(into)) >= rows {
		next.Rows = into[:rows]
	} else {
		next.Rows = make([]packedRow, int(rows))
	}
	next.Strings = make([]string, int(strs))
	next.Bytes = int64(len(next.Rows))*int64(unsafe.Sizeof(packedRow{})) + codexActorBytes(next.codexActors)
	// The strings are copied out in one piece: one allocation a block, not
	// one a string.
	at := make([]int, 0, 2*int(strs))
	section := data
	for range next.Strings {
		n := take()
		if n > uint64(len(data)) {
			return nil
		}
		at = append(at, len(section)-len(data), int(n))
		data = data[n:]
	}
	if !valid {
		return nil
	}
	text := string(section[:len(section)-len(data)])
	for i := range next.Strings {
		from, n := at[2*i], at[2*i+1]
		next.Strings[i] = text[from : from+n]
		next.Bytes += int64(n) + 16
	}
	for i := range next.Rows {
		p := &next.Rows[i]
		sec, nano, loc := int64(take()), int64(take()), take()
		if loc >= uint64(len(c.Locations)) {
			return nil
		}
		p.Time = time.Unix(sec, nano).In(c.Locations[loc])
		for j := range p.Text {
			n := take()
			if n >= strs {
				return nil
			}
			p.Text[j] = uint32(n)
		}
		for j := range p.Tokens {
			p.Tokens[j] = int64(take())
		}
		p.Millis, p.TTFT, p.FirstText, p.Order = int64(take()), int64(take()), int64(take()), int64(take())
		p.RouteID = int64(take())
		p.Sent, p.Flow = int64(take()), int64(take())
		p.Own, p.Retried, p.Queued = int64(take()), int64(take()), int64(take())
		p.FirstReasoning = int64(take())
		p.Cost, p.Status, p.Flags = math.Float64frombits(take()), int32(take()), uint16(take())
		p.Tries = int32(take())
		p.Credits = 0
		if p.Flags&rowCredits != 0 {
			p.Credits = math.Float64frombits(take())
		}
	}
	if !valid || len(data) != 0 {
		return nil
	}
	return &next
}
