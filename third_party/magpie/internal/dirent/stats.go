package dirent

import (
	"cmp"
	"io/fs"
	"slices"
	"time"
)

// Stat is what a folder's listing says of one entry: its kind (Mode's type
// bits, 0 for a regular file), and a regular file's size and modification
// time, as os.Lstat gives them.
type Stat struct {
	Mode fs.FileMode
	Size int64
	Mod  time.Time
}

// Entry is one entry of a Listing.
type Entry struct {
	Name string
	Stat
}

// Listing is a folder's entries in name order. It is a slice, not a map:
// every 15 s Sessions poll lists every Codex day folder and Claude Code
// project again, and a map of each folder's entries was ~90 MB of the 2.3 GB
// eight polls allocated.
type Listing []Entry

// Get is name's entry.
func (l Listing) Get(name string) (Stat, bool) {
	if i, ok := slices.BinarySearchFunc(l, name, func(e Entry, name string) int { return cmp.Compare(e.Name, name) }); ok {
		return l[i].Stat, true
	}
	return Stat{}, false
}

// Stats is each entry of dir in name order, as os.Lstat gives it, read in
// a few calls for the whole folder where the system has them
// (getattrlistbulk on macOS), rather than one per entry. ok is false where
// the system has none or the folder couldn't be read, and the caller asks
// after each entry itself.
func Stats(dir string) (Listing, bool) {
	l, ok := stats(dir)
	if !ok {
		return nil, false
	}
	slices.SortFunc(l, func(a, b Entry) int { return cmp.Compare(a.Name, b.Name) })
	return l, true
}
