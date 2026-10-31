package usage

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"hash"
	"math"
	"os"
	"path/filepath"
	"time"

	"github.com/yetone/magpie/internal/sessions"
)

// The heatmap's settled days are kept on disk between runs (heat-v1.json).
// A started magpie paired the whole history for them, every gateway call
// with its session row, and so again every heatOldFresh: the cold start's
// peak, ~100 MB of a ~450 MB live heap, and seconds of CPU each time
// (#2278). They stand while what they were counted of is the same, which
// heatSettledKey names: the settled chunks, the recent chunks' rows before
// the cut, the threads' identities, the filter and the days, the code that
// counts them and what prices and names a call.

// settledCountVersion names how the settled days, a page's cells before
// the cut (page-v1.json) and Codex's effort (effort-v1.json) are counted.
// They were keyed by this package's source, as requests-v1.bin is: 14 of
// 39 releases changed it, and each such upgrade paired every user's whole
// history again on its first start, the cold start's peak (#2278). Raise
// it with any change to what a row counts as; TestSettledCountedAsVersioned
// fails until it is raised.
const settledCountVersion = "1"

func heatDiskPath() string {
	return filepath.Join(filepath.Dir(sessions.CachePath()), "heat-v1.json")
}

// heatDiskEntry is the file: the key it was counted under and the days.
type heatDiskEntry struct {
	Key  string `json:"key"`
	Days []Day  `json:"days"`
}

// heatDiskRead is the days the file holds under key, or false.
func heatDiskRead(key string) ([]Day, bool) {
	b, err := os.ReadFile(heatDiskPath())
	if err != nil {
		return nil, false
	}
	var e heatDiskEntry
	if json.Unmarshal(b, &e) != nil || e.Key != key {
		return nil, false
	}
	return e.Days, true
}

// heatDiskWrite keeps days under key, through a file renamed over the last.
func heatDiskWrite(key string, days []Day) error {
	b, err := json.Marshal(heatDiskEntry{Key: key, Days: days})
	if err != nil {
		return err
	}
	path := heatDiskPath()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	out, err := os.CreateTemp(filepath.Dir(path), ".heat-*.json")
	if err != nil {
		return err
	}
	defer os.Remove(out.Name())
	_, err = out.Write(b)
	if closeErr := out.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(out.Name(), path)
}

// heatSettledKey names what the days before cut were counted of. A
// session file's chunk is named by its path, size and mtime under the
// same root and meta, any other chunk by its rows, and the counting by
// the call shards' version and settledCountVersion. False: the chunks
// can't be named so (a probe's chunk made again for each query).
func heatSettledKey(d heatDiskRef, cut, since time.Time, f Filter, identities map[string]sessions.CodexIdentity, settled [3][]*rowChunk, before heatSum) (string, bool) {
	if d.root == "" {
		return "", false
	}
	// the filter's zone by its name: %#v writes its pointer, another in
	// each run
	zone := ""
	if f.Zone != nil {
		zone = f.Zone.String()
	}
	f.Zone = nil
	h := sha256.New()
	fmt.Fprintf(h, "heat-v1\x00%s\x00%s\x00%s\x00%d\x00%d\x00%s\x00%#v\x00%s\x00%d:%d:%d\x00",
		sessions.CallCacheVersion+"\x00"+settledCountVersion, d.root, d.meta, cut.UnixNano(), since.UnixNano(), cut.Location(), f, zone, before.n, before.at, before.tokens)
	hashCodexIdentities(h, identities)
	var b []byte
	rows := lentRows()
	defer rows.release()
	for k, set := range settled {
		for _, c := range set {
			if c.serial == 0 {
				return "", false
			}
			b = fmt.Appendf(b[:0], "\x01%d\x00%s\x00%d\x00%d\x00%s\x00%d\x00", k, c.Computer, c.Count, c.Latest.UnixNano(), c.Source.Path, c.Source.Size)
			if c.Source.Path != "" {
				b = binary.AppendVarint(b, c.Source.Modified.UnixNano())
				h.Write(b)
				continue
			}
			h.Write(b)
			if c.raw != nil {
				// a gateway block by its rows as logged: meta names what
				// prices them, and one not priced has no rows of its own
				hashRows(h, rows.of(c.raw))
				continue
			}
			hashRows(h, rows.of(c))
		}
	}
	return fmt.Sprintf("%x", h.Sum(nil)), true
}

// hashRows writes an unpacked chunk's rows into h, each field as a number
// and each string as written: a row's time holds a pointer, its padding
// isn't written by every copy, and a chunk packed and the same unpacked
// number their strings and shapes apart. A shape is written where a row
// first has it, and by that row after.
func hashRows(h hash.Hash, c *rowChunk) {
	b := make([]byte, 0, 32<<10)
	text := func(i uint32) {
		s := ""
		if int(i) < len(c.Strings) {
			s = c.Strings[i]
		}
		b = binary.AppendUvarint(b, uint64(len(s)))
		b = append(b, s...)
	}
	shapes := map[uint32]int{}
	for i := range c.Rows {
		p := &c.Rows[i]
		b = binary.AppendVarint(b, p.Time.UnixNano())
		for _, v := range p.Varied {
			text(v)
		}
		if first, ok := shapes[p.Shape]; ok {
			b = binary.AppendUvarint(b, uint64(first))
		} else {
			shapes[p.Shape] = i
			b = binary.AppendUvarint(b, uint64(i))
			if int(p.Shape) < len(c.Shapes) {
				for _, v := range c.Shapes[p.Shape] {
					text(v)
				}
			}
		}
		for _, t := range p.Tokens {
			b = binary.AppendVarint(b, t)
		}
		b = binary.AppendVarint(b, p.Order)
		b = binary.AppendVarint(b, p.RouteID)
		b = binary.AppendUvarint(b, math.Float64bits(p.Cost))
		b = binary.AppendUvarint(b, math.Float64bits(p.Credits))
		for _, v := range [...]int32{p.Millis, p.TTFT, p.FirstText, p.Sent, p.Flow, p.FirstReasoning, p.Own, p.Retried, p.Queued, p.Masking, p.Status, p.Tries} {
			b = binary.AppendVarint(b, int64(v))
		}
		b = binary.AppendUvarint(b, uint64(p.Flags))
		if len(b) > 30<<10 {
			h.Write(b)
			b = b[:0]
		}
	}
	h.Write(b)
}

// heatDiskRef is where a heatmap's settled days are kept and what prices
// and names its calls: the index's root and meta. Empty, they aren't kept.
type heatDiskRef struct {
	root, meta string
}
