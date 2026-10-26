package pii

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"errors"
	"io"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/redact"
)

// What Find learned is kept on disk too, so that a restart doesn't ask the
// model about every history again: after one, each request's whole
// history was a miss, and on a CPU the requests queued for minutes behind
// each other (#3682). The file holds a text's sha256, and where in it the
// model found what, with the label: never the text. It is named for the
// model it came from, so another model's answers are never read as this
// one's, and lives with the model, in magpie's cache (Remove deletes it).

// cacheMagic starts the file; a file that doesn't start with it, or ends
// part way, is read as empty.
const cacheMagic = "pfound01"

func cachePath() string { return filepath.Join(dir(), "found-"+modelSHA[:12]+".bin") }

// saveAfter is how long after an answer the file is written: one write
// for a burst of requests.
var saveAfter = 2 * time.Second

var disk struct {
	mu     sync.Mutex
	loaded string // the file read into cache, once
	timer  *time.Timer
	saving sync.Mutex
}

// loadCache reads the file into cache, the first time Find is asked.
func loadCache() {
	disk.mu.Lock()
	defer disk.mu.Unlock()
	p := cachePath()
	if disk.loaded == p {
		return
	}
	disk.loaded = p
	b, err := os.ReadFile(p)
	if err != nil {
		return
	}
	es, err := decodeCache(b)
	if err != nil {
		return // a file cut short or not ours: nothing is kept, the model asked again
	}
	cache.mu.Lock()
	defer cache.mu.Unlock()
	for _, e := range es {
		if _, ok := cache.m[e.k]; !ok {
			cache.put(e.k, e.fs)
		}
	}
}

// saveSoon writes the file saveAfter from now, unless a write is due.
func saveSoon() {
	disk.mu.Lock()
	defer disk.mu.Unlock()
	if disk.timer == nil {
		disk.timer = time.AfterFunc(saveAfter, func() {
			disk.mu.Lock()
			disk.timer = nil
			disk.mu.Unlock()
			saveCache()
		})
	}
}

// saveCache writes what cache holds, oldest first, to a file beside the
// old one and renames it over it: a reader never sees half of it.
func saveCache() error {
	disk.saving.Lock()
	defer disk.saving.Unlock()
	cache.mu.Lock()
	b := encodeCache(cache)
	cache.mu.Unlock()
	p := cachePath()
	f, err := os.CreateTemp(filepath.Dir(p), "found-*.tmp")
	if err != nil {
		return err
	}
	_, err = f.Write(b)
	if err == nil {
		err = f.Sync()
	}
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Rename(f.Name(), p)
	}
	if err != nil {
		os.Remove(f.Name())
	}
	return err
}

// encodeCache is c, oldest first: the magic, a count, then each entry's
// hash, its spans' count and each span's start, end and label.
func encodeCache(c *lru) []byte {
	var b bytes.Buffer
	b.WriteString(cacheMagic)
	put := func(v any) { binary.Write(&b, binary.LittleEndian, v) }
	put(uint32(c.ll.Len()))
	for e := c.ll.Back(); e != nil; e = e.Prev() {
		en := e.Value.(*entry)
		b.Write(en.k[:])
		put(uint16(len(en.fs)))
		for _, f := range en.fs {
			label := f.Label
			if len(label) > 255 {
				label = label[:255]
			}
			put(uint32(f.Start))
			put(uint32(f.End))
			b.WriteByte(byte(len(label)))
			b.WriteString(label)
		}
	}
	return b.Bytes()
}

var errBadCache = errors.New("not a cache file")

func decodeCache(b []byte) ([]entry, error) {
	if !bytes.HasPrefix(b, []byte(cacheMagic)) {
		return nil, errBadCache
	}
	r := bufio.NewReader(bytes.NewReader(b[len(cacheMagic):]))
	var n uint32
	if err := binary.Read(r, binary.LittleEndian, &n); err != nil {
		return nil, err
	}
	if n > uint32(len(b)/34) { // each entry is 34 bytes at least
		return nil, errBadCache
	}
	out := make([]entry, 0, n)
	for i := uint32(0); i < n; i++ {
		var e entry
		if _, err := io.ReadFull(r, e.k[:]); err != nil {
			return nil, err
		}
		var spans uint16
		if err := binary.Read(r, binary.LittleEndian, &spans); err != nil {
			return nil, err
		}
		for j := uint16(0); j < spans; j++ {
			var se [2]uint32
			if err := binary.Read(r, binary.LittleEndian, &se); err != nil {
				return nil, err
			}
			l, err := r.ReadByte()
			if err != nil {
				return nil, err
			}
			label := make([]byte, l)
			if _, err := io.ReadFull(r, label); err != nil {
				return nil, err
			}
			if se[0] >= se[1] {
				return nil, errBadCache
			}
			e.fs = append(e.fs, redact.Found{Start: int(se[0]), End: int(se[1]), Label: string(label)})
		}
		out = append(out, e)
	}
	if _, err := r.ReadByte(); err != io.EOF {
		return nil, errBadCache // more than it said
	}
	return out, nil
}
