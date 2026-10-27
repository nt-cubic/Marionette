package sessions

import (
	"slices"
	"sync"
	"time"
)

// held is the session folders' listings kept while a request that reads
// them more than once is answered. A Sessions poll lists every Claude
// Code and Codex file for its list (allFiles), then again for the Codex
// threads' effort (the Requests state's CallSources): ~21k files, read
// twice a poll.
var held struct {
	sync.Mutex
	n     int
	since time.Time // when the latest hold began
	lists map[string]*heldList
}

type heldList struct {
	at    time.Time
	files []file
	// closed once files is in: a read that comes while the folder is
	// being listed waits for that listing rather than taking its own
	ready chan struct{}
}

// OneListing has the session folders listed once until done is called:
// the reads between share each folder's listing, taken after the latest
// hold began, so none sees the files as they were before its own request
// came. done lets the listings go once no hold is left.
func OneListing() (done func()) {
	held.Lock()
	held.n++
	held.since = time.Now()
	held.Unlock()
	return letGo()
}

// ShareListing is OneListing for a read that may see the listings the
// holds there are now share, taken before it came: it begins a hold only
// when there is none. The Usage page's Sessions tab asks for the list,
// the stats and the overview at once, and the stats, read again behind
// (statsFor), listed every session file a second time beside the list's
// own listing.
func ShareListing() (done func()) {
	held.Lock()
	defer held.Unlock()
	if held.n++; held.n == 1 {
		held.since = time.Now()
	}
	return letGo()
}

func letGo() func() {
	return sync.OnceFunc(func() {
		held.Lock()
		if held.n--; held.n == 0 {
			held.lists = nil
		}
		held.Unlock()
	})
}

// heldWaiting is a test's: called as a read waits for another's listing.
var heldWaiting func()

// heldFiles is list(), or, under a hold, the listing of key taken since
// the latest hold began. The folders are listed outside the lock, once
// for the reads that ask meanwhile.
func heldFiles(key string, list func() []file) []file {
	held.Lock()
	if held.n == 0 {
		held.Unlock()
		return list()
	}
	if l, ok := held.lists[key]; ok && !l.at.Before(held.since) {
		held.Unlock()
		if heldWaiting != nil {
			heldWaiting()
		}
		<-l.ready
		return slices.Clone(l.files)
	}
	mine := &heldList{at: time.Now(), ready: make(chan struct{})}
	if held.lists == nil {
		held.lists = map[string]*heldList{}
	}
	held.lists[key] = mine
	held.Unlock()
	defer close(mine.ready)
	files := list()
	// before ready is closed, so whoever waited reads it
	mine.files = slices.Clone(files)
	return files
}
