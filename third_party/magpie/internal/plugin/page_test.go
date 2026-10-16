package plugin

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// A plugin's page (magpie.page) is a file inside the plugin's folder: one
// that names a path out of it, by "..", an absolute path or a symbolic
// link, is no page. Its reads are those magpie has, and a plugin that is
// only a page is nothing for the host to load, nor "not a plugin".
func TestWindowPageStaysInside(t *testing.T) {
	root := t.TempDir()
	outside := filepath.Join(root, "secret.html")
	if err := os.WriteFile(outside, []byte("<p>secret</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	mk := func(name, pkg string) string {
		t.Helper()
		d := filepath.Join(root, name)
		if err := os.MkdirAll(filepath.Join(d, "page"), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(d, "page", "index.html"), []byte("<p>hi</p>"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(d, "package.json"), []byte(pkg), 0o644); err != nil {
			t.Fatal(err)
		}
		return d
	}
	good := mk("good", `{"name":"good","magpie":{"page":{"title":"Good","file":"page/index.html","icon":"page/index.html"},"permissions":["usage:summary","secrets:all","usage:summary"]}}`)
	p, only := WindowPage(good)
	if p == nil || !only || p.Title != "Good" || p.Icon != "index.html" {
		t.Fatalf("good: %+v only=%v", p, only)
	}
	if !slices.Equal(p.Reads, []string{"usage:summary"}) {
		t.Errorf("reads %v, want only the one magpie has, once", p.Reads)
	}
	if !InMagpieOnly(good) || notPlugin(good) != nil {
		t.Errorf("a page-only plugin: InMagpieOnly %v, notPlugin %v", InMagpieOnly(good), notPlugin(good))
	}
	if _, only := WindowPage(mk("main", `{"name":"m","main":"index.js","magpie":{"page":{"file":"page/index.html"}}}`)); only {
		t.Error("a plugin with a main is more than its page")
	}
	link := mk("link", `{"name":"l","magpie":{"page":{"file":"page/out.html"}}}`)
	if err := os.Symlink(outside, filepath.Join(link, "page", "out.html")); err != nil {
		t.Fatal(err)
	}
	for name, pkg := range map[string]string{
		"dotdot": `{"name":"d","magpie":{"page":{"file":"../secret.html"}}}`,
		"abs":    `{"name":"a","magpie":{"page":{"file":"` + filepath.ToSlash(outside) + `"}}}`,
		"notml":  `{"name":"n","magpie":{"page":{"file":"package.json"}}}`,
		"none":   `{"name":"x","magpie":{"page":{"file":"page/missing.html"}}}`,
	} {
		if p, _ := WindowPage(mk(name, pkg)); p != nil {
			t.Errorf("%s: %+v is a page", name, p)
		}
	}
	if p, _ := WindowPage(link); p != nil {
		t.Errorf("a symbolic link out of the folder: %+v is a page", p)
	}
	if f, ok := p.PageFile("../package.json"); ok {
		t.Errorf("PageFile served %s", f)
	}
	if f, ok := p.PageFile("index.html"); !ok || filepath.Base(f) != "index.html" {
		t.Errorf("PageFile(index.html) = %s %v", f, ok)
	}
}
