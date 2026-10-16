package settings

import (
	"os"
	"path/filepath"
	"testing"
)

// An empty folder is a new install; any one of ranBefore's files says a
// magpie ran here before.
func TestRanBefore(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	if RanBefore() {
		t.Fatal("an empty folder ran before")
	}
	for _, name := range ranBefore {
		t.Run(name, func(t *testing.T) {
			t.Setenv("XDG_CONFIG_HOME", t.TempDir())
			if err := os.MkdirAll(Dir(), 0o700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(Dir(), name), []byte("{}"), 0o600); err != nil {
				t.Fatal(err)
			}
			if !RanBefore() {
				t.Fatalf("%s there, and not ran before", name)
			}
		})
	}
}
