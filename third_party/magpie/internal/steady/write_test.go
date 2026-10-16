package steady

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

// A file is flushed to the disk before it is renamed over the old one: a
// Windows machine that went down between the two left logins.json at its
// full length, every byte zero (#1505). The flush is of the new contents,
// before the old file is replaced; one that fails leaves the old file and
// no temp file beside it.
func TestWriteFileSyncsBeforeRename(t *testing.T) {
	defer func() { Sync = (*os.File).Sync }()
	dir := t.TempDir()
	p := filepath.Join(dir, "logins.json")
	if err := os.WriteFile(p, []byte(`["old"]`), 0o600); err != nil {
		t.Fatal(err)
	}

	synced := 0
	Sync = func(f *os.File) error {
		synced++
		if b, _ := os.ReadFile(p); string(b) != `["old"]` {
			t.Errorf("flushed after the rename: the file was already %q", b)
		}
		if b, _ := os.ReadFile(f.Name()); string(b) != `["new"]` {
			t.Errorf("flushed %q, want the new contents", b)
		}
		return f.Sync()
	}
	if err := WriteFile(p, []byte(`["new"]`), 0o600); err != nil {
		t.Fatal(err)
	}
	if synced != 1 {
		t.Fatalf("flushed %d times, want once", synced)
	}
	if b, _ := os.ReadFile(p); string(b) != `["new"]` {
		t.Fatalf("file = %q", b)
	}

	Sync = func(*os.File) error { return errors.New("the disk went away") }
	if err := WriteFile(p, []byte(`["newer"]`), 0o600); err == nil {
		t.Fatal("a flush that failed reported as written")
	}
	if b, _ := os.ReadFile(p); string(b) != `["new"]` {
		t.Fatalf("a failed flush changed the file to %q", b)
	}
	if left, _ := os.ReadDir(dir); len(left) != 1 {
		t.Fatalf("a failed write left %v", left)
	}
}
