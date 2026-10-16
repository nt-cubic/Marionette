package plugin

import (
	"os"
	"path/filepath"
	"testing"
)

// plugins.json zeroed by a crash (#1505) is its last good generation: the
// plugins installed are not none.
func TestZeroedPluginListReadFromBackup(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, "config"))
	os.MkdirAll(filepath.Dir(listPath()), 0o700)
	for _, l := range []List{{Plugins: []Entry{{Spec: "a"}}}, {Plugins: []Entry{{Spec: "a"}, {Spec: "b"}}}} {
		if err := save(l); err != nil {
			t.Fatal(err)
		}
	}
	os.WriteFile(listPath(), make([]byte, 300), 0o600)
	if l := list(); len(l.Plugins) != 1 || l.Plugins[0].Spec != "a" {
		t.Fatalf("a zeroed plugins.json read as %+v", l.Plugins)
	}
}
