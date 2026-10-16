package settings

import (
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"

	"github.com/yetone/magpie/internal/lastgood"
)

// LoadChecked tells settings that aren't there (no error) from settings
// nobody knows (an error): a file that can't be read, or doesn't parse
// with no last good generation. Load still reads the defaults for those,
// and Save refuses the same files it reports.
func TestLoadChecked(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	lastgood.Reset()
	t.Cleanup(lastgood.Reset)

	if s, err := LoadChecked(); err != nil || len(s.AllowIPs) != 0 {
		t.Fatalf("no file: %v %v", s.AllowIPs, err)
	}
	if err := os.MkdirAll(Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(Path(), []byte(" \n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadChecked(); err != nil {
		t.Fatalf("an empty file: %v", err)
	}

	os.Remove(Path())
	for _, list := range [][]string{{"10.0.0.0/8"}, {"198.51.100.7"}} {
		if err := Save(Settings{AllowIPs: list}); err != nil {
			t.Fatal(err)
		}
	}
	if s, err := LoadChecked(); err != nil || len(s.AllowIPs) != 1 || s.AllowIPs[0] != "198.51.100.7" {
		t.Fatalf("a good file: %v %v", s.AllowIPs, err)
	}

	// garbled, with its last good generation: that is read, no error
	garbled := []byte(`{"allowIPs": ["198.51.100.7"`)
	if err := os.WriteFile(Path(), garbled, 0o600); err != nil {
		t.Fatal(err)
	}
	if s, err := LoadChecked(); err != nil || len(s.AllowIPs) != 1 || s.AllowIPs[0] != "10.0.0.0/8" {
		t.Fatalf("garbled with a backup: %v %v", s.AllowIPs, err)
	}

	// and without one: unknown, and never saved over
	if err := os.Remove(lastgood.Bak(Path())); err != nil {
		t.Fatal(err)
	}
	if s, err := LoadChecked(); err == nil {
		t.Fatalf("garbled with no backup read as good: %v", s.AllowIPs)
	}
	if s := Load(); !reflect.DeepEqual(s, Settings{}.normal()) {
		t.Fatalf("Load of a garbled file changed: %+v", s)
	}
	if err := Save(Load()); err == nil {
		t.Fatal("Save wrote the defaults over a garbled settings.json")
	}
	if b, _ := os.ReadFile(Path()); string(b) != string(garbled) {
		t.Fatalf("the garbled file was written over: %q", b)
	}

	// one that can't be read at all
	if runtime.GOOS == "windows" {
		return
	}
	if err := os.WriteFile(Path(), []byte(`{"allowIPs":["10.0.0.0/8"]}`), 0o200); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(Path(), 0o200); err != nil {
		t.Fatal(err)
	}
	if _, err := os.ReadFile(Path()); err == nil {
		t.Skip("this process can read a write-only file")
	}
	if _, err := LoadChecked(); err == nil {
		t.Fatal("an unreadable file read as good")
	}
}
