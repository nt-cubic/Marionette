package plugin

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
)

// dirLink makes path a link to the folder target: a junction on Windows,
// what Scoop hangs magpie's data folder off its install folder with, and a
// symlink elsewhere.
func dirLink(t *testing.T, target, path string) {
	t.Helper()
	if runtime.GOOS == "windows" {
		if out, err := exec.Command("cmd", "/c", "mklink", "/J", path, target).CombinedOutput(); err != nil {
			t.Fatalf("mklink /J: %v %s", err, out)
		}
		return
	}
	if err := os.Symlink(target, path); err != nil {
		t.Fatal(err)
	}
}

// A config dir reached through a link (Scoop's persist junction, #4265)
// holds the very files the real one does, so its plugin's page registers
// and its files are served all the same. A link inside the page's folder
// that leads out of it is still turned away. On Windows the links are real
// junctions, which filepath.EvalSymlinks gives up on since Go 1.23.
func TestWindowPageThroughLinkedConfigDir(t *testing.T) {
	root := t.TempDir()
	real := filepath.Join(root, "persist", "data")
	pkg := filepath.Join(real, "plugins", "node_modules", "github.com", "owner", "chat")
	if err := os.MkdirAll(filepath.Join(pkg, "page"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pkg, "package.json"), []byte(`{"name":"chat","main":"host.js","magpie":{"page":{"title":"Chat","file":"page/index.html"},"permissions":["usage:summary"]}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(pkg, "page", "index.html"), []byte("<p>hi</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	secret := filepath.Join(root, "secret")
	if err := os.MkdirAll(secret, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(secret, "index.html"), []byte("<p>no</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	dirLink(t, secret, filepath.Join(pkg, "page", "out"))

	// apps\magpie-ai\<ver>\data → persist\data, as Scoop lays it out
	app := filepath.Join(root, "apps", "1.0")
	if err := os.MkdirAll(app, 0o755); err != nil {
		t.Fatal(err)
	}
	dirLink(t, real, filepath.Join(app, "data"))
	// and the exe started through apps\magpie-ai\current → <ver>
	dirLink(t, app, filepath.Join(root, "apps", "current"))

	for _, data := range []string{filepath.Join(app, "data"), filepath.Join(root, "apps", "current", "data")} {
		target := filepath.Join(data, "plugins", "node_modules", "github.com", "owner", "chat")
		w, only := Window(target)
		if w == nil || w.Page == nil || w.Page.Title != "Chat" || only {
			t.Fatalf("Window through %s = %+v only=%v, want the page", data, w, only)
		}
		if f, ok := FileIn(w.Page.Dir, "index.html"); !ok || filepath.Base(f) != "index.html" {
			t.Errorf("FileIn through %s = %q %v", data, f, ok)
		}
		if f, ok := FileIn(w.Page.Dir, "out/index.html"); ok {
			t.Errorf("a link out of the page's folder served %q", f)
		}
	}
}
