package sessions

import (
	"sync/atomic"
	"testing"
)

// Under a hold a folder is listed once; a hold begun after a listing has
// it listed again, so no request reads files from before it came; with
// no hold left every read lists.
func TestOneListing(t *testing.T) {
	n := 0
	list := func() []file { n++; return []file{{path: "a"}} }
	read := func() {
		if fs := heldFiles("k", list); len(fs) != 1 || fs[0].path != "a" {
			t.Fatalf("heldFiles = %v", fs)
		}
	}
	read()
	read()
	if n != 2 {
		t.Fatalf("listed %d times with no hold, want 2", n)
	}
	n = 0
	done := OneListing()
	read()
	read()
	if n != 1 {
		t.Fatalf("listed %d times under one hold, want 1", n)
	}
	other := OneListing()
	read()
	if n != 2 {
		t.Fatalf("a hold begun after the listing read it: listed %d times, want 2", n)
	}
	read()
	done()
	read()
	if n != 2 {
		t.Fatalf("listed %d times while a hold was left, want 2", n)
	}
	other()
	other()
	read()
	if n != 3 {
		t.Fatalf("listed %d times after the holds ended, want 3", n)
	}
	if held.n != 0 || held.lists != nil {
		t.Fatalf("holds left: %d, lists %v", held.n, held.lists)
	}
}

// A read that comes while a folder is being listed under a hold waits for
// that listing; a shared hold reads the listing a hold before it took, and
// begins one of its own when there is none.
func TestShareListing(t *testing.T) {
	var n atomic.Int32
	listing, release := make(chan struct{}), make(chan struct{})
	slow := func() []file {
		if n.Add(1) == 1 {
			close(listing)
			<-release
		}
		return []file{{path: "a"}}
	}
	waiting := make(chan struct{}, 1)
	heldWaiting = func() { waiting <- struct{}{} }
	t.Cleanup(func() { heldWaiting = nil })
	done := OneListing()
	got := make(chan []file, 2)
	go func() { got <- heldFiles("k", slow) }()
	<-listing
	shared := ShareListing()
	go func() { got <- heldFiles("k", slow) }()
	select {
	case <-waiting:
	case <-got:
		t.Fatal("a read during the listing didn't wait for it")
	}
	close(release)
	for range 2 {
		if fs := <-got; len(fs) != 1 || fs[0].path != "a" {
			t.Fatalf("heldFiles = %v", fs)
		}
	}
	if n.Load() != 1 {
		t.Fatalf("listed %d times for a read during the listing and a shared hold, want 1", n.Load())
	}
	done()
	shared()

	n.Store(1) // listing again is quick
	shared = ShareListing()
	heldFiles("k", slow)
	heldFiles("k", slow)
	shared()
	if n.Load() != 2 {
		t.Fatalf("a shared hold with none before it listed %d times, want 1", n.Load()-1)
	}
	if held.n != 0 || held.lists != nil {
		t.Fatalf("holds left: %d, lists %v", held.n, held.lists)
	}
}
