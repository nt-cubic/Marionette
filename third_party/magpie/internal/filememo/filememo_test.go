package filememo

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestRead(t *testing.T) {
	p := filepath.Join(t.TempDir(), "f")
	os.WriteFile(p, []byte("a"), 0o600)
	n := 0
	parse := func(b []byte) (string, error) { n++; return string(b), nil }
	for range 3 {
		if v, err := Read("t", p, parse); err != nil || v != "a" {
			t.Fatal(v, err)
		}
	}
	if n != 1 {
		t.Fatalf("parsed %d times", n)
	}
	os.WriteFile(p, []byte("bb"), 0o600)
	os.Chtimes(p, time.Now(), time.Now().Add(time.Second))
	if v, _ := Read("t", p, parse); v != "bb" || n != 2 {
		t.Fatalf("after a change: %q, parsed %d times", v, n)
	}
	os.Remove(p)
	if _, err := Read("t", p, parse); err == nil {
		t.Fatal("a file gone is an error")
	}
}

// two writes of one size inside one tick of the file system's clock (Linux
// stamps a file's time from a clock a few milliseconds coarse) leave the
// same time and size: the second is still read, not the first's parse
func TestReadSameSizeSameTick(t *testing.T) {
	p := filepath.Join(t.TempDir(), "f")
	parse := func(b []byte) (string, error) { return string(b), nil }
	for i := range 200 {
		a, b := fmt.Sprintf("a%03d,b", i), fmt.Sprintf("b%03d,a", i)
		os.WriteFile(p, []byte(a), 0o600)
		if v, _ := Read("tick", p, parse); v != a {
			t.Fatalf("read %q, want %q", v, a)
		}
		os.WriteFile(p, []byte(b), 0o600)
		if v, _ := Read("tick", p, parse); v != b {
			t.Fatalf("read %q after writing %q: the first write's parse", v, b)
		}
	}
}

// While a request holds what was read, a file found unchanged is not looked
// at again (the GUI's state looked a few thousand times, seconds on a slow
// disk); a write magpie makes (Forget), a second gone by or the release has
// it looked at, and a file just written is always read again.
func TestHold(t *testing.T) {
	p := filepath.Join(t.TempDir(), "f")
	os.WriteFile(p, []byte("a"), 0o600)
	old := time.Now().Add(-time.Minute)
	os.Chtimes(p, old, old)
	parse := func(b []byte) (string, error) { return string(b), nil }
	write := func(s string) {
		os.WriteFile(p, []byte(s), 0o600)
		at := old.Add(time.Duration(len(s)) * time.Second)
		os.Chtimes(p, at, at)
	}
	release := Hold()
	if v, _ := Read("hold", p, parse); v != "a" {
		t.Fatal(v)
	}
	write("bb")
	if v, _ := Read("hold", p, parse); v != "a" {
		t.Fatalf("looked at again while held: %q", v)
	}
	Forget()
	if v, _ := Read("hold", p, parse); v != "bb" {
		t.Fatalf("after a write magpie made: %q", v)
	}
	write("ccc")
	release()
	release()
	if v, _ := Read("hold", p, parse); v != "ccc" {
		t.Fatalf("released: %q", v)
	}
	if holds != 0 {
		t.Fatalf("holds left: %d", holds)
	}
	// a file written just now is read each time, held or not
	defer Hold()()
	os.WriteFile(p, []byte("d"), 0o600)
	Read("hold", p, parse)
	os.WriteFile(p, []byte("e"), 0o600)
	if v, _ := Read("hold", p, parse); v != "e" {
		t.Fatalf("a file written just now: %q", v)
	}
}
