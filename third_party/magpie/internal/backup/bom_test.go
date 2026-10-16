package backup

import (
	"errors"
	"os"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/edit"
)

// A backup file an editor saved over with a byte order mark (a Windows
// editor's "UTF-8 with BOM", PowerShell 5.1's Set-Content) still opens:
// the mark sits in front of the sealed JSON, and the write through
// edit.WriteAtomic keeps the file's mark (#1534), so the reader trims it.
// A file that isn't a backup at all stays that, mark or not, and one cut
// short is still a damaged backup a sync may rebuild.
func TestOpenReadsThroughABOM(t *testing.T) {
	home(t)
	setUp(t)
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, err := Seal(b, "correct horse")
	if err != nil {
		t.Fatal(err)
	}
	mark := []byte("\xef\xbb\xbf")
	got, err := Open(append(append([]byte(nil), mark...), data...), "correct horse")
	if err != nil {
		t.Fatalf("a backup with a byte order mark: %v", err)
	}
	if len(got.Providers) != len(b.Providers) {
		t.Fatalf("providers: %d, want %d", len(got.Providers), len(b.Providers))
	}
	if _, err := Open(append(append([]byte(nil), mark...), []byte("<html>sign in</html>")...), "correct horse"); !errors.Is(err, ErrNotBackup) {
		t.Fatalf("a marked page that is no backup: %v", err)
	}
	if _, err := Open(append(append([]byte(nil), mark...), data[:len(data)/2]...), "correct horse"); !errors.Is(err, ErrCorrupt) {
		t.Fatalf("a marked backup cut short: %v", err)
	}
}

// magpie's own round trip: `magpie backup f` writes through edit.WriteAtomic,
// which keeps the mark of a file already at f, and `magpie restore f` reads
// the file as it is on disk.
func TestBackupSavedOverAMarkedFileRestores(t *testing.T) {
	home(t)
	setUp(t)
	b, err := Collect(true, "test")
	if err != nil {
		t.Fatal(err)
	}
	data, err := Seal(b, "correct horse")
	if err != nil {
		t.Fatal(err)
	}
	f := filepath.Join(t.TempDir(), "old"+Ext)
	if err := os.WriteFile(f, []byte("\xef\xbb\xbf{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := edit.WriteAtomic(f, data); err != nil {
		t.Fatal(err)
	}
	on, err := os.ReadFile(f)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := Open(on, "correct horse"); err != nil {
		t.Fatalf("a backup saved over a file with a byte order mark: %v", err)
	}
}
