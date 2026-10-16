// Package filememo keeps what was parsed from a file until the file
// changes, so a page that asks many times over reads it once.
package filememo

import (
	"bytes"
	"errors"
	"io/fs"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/steady"
)

type entry struct {
	mod  time.Time
	size int64
	v    any
	// the bytes parsed, while the file's time is too recent to tell a
	// second write by: Linux stamps a file from a clock a tick (a few
	// milliseconds) coarse, so two writes of one size inside a tick (two
	// accounts swapped) leave the same time and size
	raw []byte
	// when the file was last found unchanged, while a request held them
	// (Hold), and in which of Forget's generations
	checked time.Time
	gen     uint64
}

// settled is how old a file's time must be for any later write to stamp
// a later one, many ticks of the coarsest clock.
const settled = 2 * time.Second

var (
	mu   sync.Mutex
	seen = map[string]entry{}
	// holds is how many requests hold what was read (Hold); gen is moved
	// on by every write magpie makes (Forget)
	holds int
	gen   uint64
	// gone is the files found not there last time looked at, not waited
	// on again (steady.Stat): one there before and not now may be being
	// replaced (Windows), and is
	gone = map[string]bool{}
)

// heldFor is how long a file found unchanged is taken as unchanged, while
// a request holds what was read, before it is looked at again.
const heldFor = time.Second

// Hold has a file found unchanged not looked at again, for a request that
// only reads, until release is called or a second has gone by: the GUI's
// state looked at each agent's files hundreds of times over, a few
// thousand looks at the disk that a slow one (a Mac's antivirus looking
// at each) made seconds. A write meanwhile (Forget) has them looked at.
func Hold() (release func()) {
	mu.Lock()
	holds++
	mu.Unlock()
	return sync.OnceFunc(func() {
		mu.Lock()
		holds--
		mu.Unlock()
	})
}

// Forget has every file looked at again on its next read: magpie wrote
// one.
func Forget() {
	mu.Lock()
	gen++
	mu.Unlock()
}

// Look has the file at path looked at on its next read, even while a
// request holds what was read (Hold): another magpie may have written it,
// which Forget, moved on by this one's writes alone, can't know of.
func Look(path string) {
	mu.Lock()
	defer mu.Unlock()
	for k, e := range seen {
		if strings.HasSuffix(k, "\x00"+path) && !e.checked.IsZero() {
			e.checked = time.Time{}
			seen[k] = e
		}
	}
}

// Read is parse of the file at path, from what it gave last time when the
// file has the same size and time as then (and, written just now, the same
// bytes). kind tells apart two parses of
// one file. What it returns is shared: the caller must not change it.
func Read[T any](kind, path string, parse func([]byte) (T, error)) (T, error) {
	var zero T
	key := kind + "\x00" + path
	mu.Lock()
	e, ok := seen[key]
	if ok && holds > 0 && e.raw == nil && e.gen == gen && time.Since(e.checked) < heldFor {
		mu.Unlock()
		return e.v.(T), nil
	}
	g := gen
	mu.Unlock()
	fi, err := statFile(path)
	if err != nil {
		return zero, err
	}
	if ok && e.mod.Equal(fi.ModTime()) && e.size == fi.Size() && e.raw == nil {
		mu.Lock()
		if holds > 0 && g == gen {
			e.checked, e.gen = time.Now(), g
			seen[key] = e
		}
		mu.Unlock()
		return e.v.(T), nil
	}
	b, err := steady.ReadThere(path)
	if err != nil {
		return zero, err
	}
	if ok && e.mod.Equal(fi.ModTime()) && bytes.Equal(b, e.raw) {
		if time.Since(e.mod) > settled {
			mu.Lock()
			e.raw = nil
			seen[key] = e
			mu.Unlock()
		}
		return e.v.(T), nil
	}
	v, err := parse(b)
	if err != nil {
		return zero, err
	}
	mu.Lock()
	e = entry{mod: fi.ModTime(), size: fi.Size(), v: v}
	if time.Since(e.mod) <= settled {
		e.raw = b
	} else if holds > 0 && g == gen {
		e.checked, e.gen = time.Now(), g
	}
	seen[key] = e
	mu.Unlock()
	return v, nil
}

// statFile is os.Stat of path, waiting out a moment of it not there
// (another process replacing it, on Windows) unless it wasn't there last
// time either: taken for gone, a file of accounts read as no accounts, and
// the next write left them all out (Packing1 on Discord, Windows 11:
// Antigravity's account gone after an update).
func statFile(path string) (os.FileInfo, error) {
	mu.Lock()
	wasGone := gone[path]
	mu.Unlock()
	var fi os.FileInfo
	var err error
	if wasGone {
		fi, err = os.Stat(path)
	} else {
		fi, err = steady.Stat(path)
	}
	mu.Lock()
	if errors.Is(err, fs.ErrNotExist) {
		gone[path] = true
	} else {
		delete(gone, path)
	}
	mu.Unlock()
	return fi, err
}
