package plugin

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"time"
)

// Bun keeps every version it ever fetched in its install cache, and an
// install takes none out: 131 versions of the community plugins on the
// owner's Mac, one more with each release of a moved subscription's plugin
// (#1988). An install leaves the versions installed and takes out the rest.
// The layout is Bun's: scope/name@version@@registry@@@1 folders, a
// scope/name folder of links to them, @T@ tarball entries and .npm
// manifests.
func TestInstallPrunesBunsCache(t *testing.T) {
	offSandbox(t)
	if err := os.MkdirAll(Dir(), 0o755); err != nil {
		t.Fatal(err)
	}
	os.WriteFile(filepath.Join(Dir(), "package.json"), []byte(`{"name":"magpie-plugins","private":true}`), 0o644)
	cache := installCacheDir()
	scope := filepath.Join(cache, "@magpie-community")
	ver := func(name, v string) string {
		return filepath.Join(scope, name+"@"+v+"@@registry.npmjs.org@@@1")
	}
	mk := func(dir string) {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{}`), 0o644)
	}
	old, kept := ver("opencode-grok-auth", "0.1.16"), ver("opencode-grok-auth", "0.1.17")
	gone := ver("opencode-kiro-auth", "0.1.5")  // a plugin no longer installed
	unsure := ver("opencode-zed-auth", "0.1.7") // installed, its package.json unreadable
	tarball := filepath.Join(cache, "@T@2773b89432f28711@@@1")
	for _, d := range []string{old, kept, gone, unsure, tarball} {
		mk(d)
	}
	manifest := filepath.Join(cache, "48634a9ce3e2fcc1-02200d3777602379.npm")
	os.WriteFile(manifest, []byte("x"), 0o644)
	links := filepath.Join(scope, "opencode-grok-auth")
	os.MkdirAll(links, 0o755)
	if runtime.GOOS != "windows" {
		os.Symlink(old, filepath.Join(links, "0.1.16@@registry.npmjs.org@@@1"))
		os.Symlink(kept, filepath.Join(links, "0.1.17@@registry.npmjs.org@@@1"))
	}
	zed := filepath.Join(Dir(), "node_modules", "@magpie-community", "opencode-zed-auth")
	os.MkdirAll(zed, 0o755)
	os.WriteFile(filepath.Join(zed, "package.json"), []byte(`{"version":`), 0o644)

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	if err := install(ctx, "@magpie-community/opencode-grok-auth@0.1.17"); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{old, gone} {
		if _, err := os.Stat(p); !os.IsNotExist(err) {
			t.Errorf("%s left in Bun's cache, not installed: %v", filepath.Base(p), err)
		}
	}
	for _, p := range []string{kept, unsure, tarball, manifest} {
		if _, err := os.Stat(p); err != nil {
			t.Errorf("%s taken out of Bun's cache: %v", filepath.Base(p), err)
		}
	}
	if runtime.GOOS != "windows" {
		if _, err := os.Lstat(filepath.Join(links, "0.1.16@@registry.npmjs.org@@@1")); !os.IsNotExist(err) {
			t.Errorf("the link to the version taken out left: %v", err)
		}
		if _, err := os.Stat(filepath.Join(links, "0.1.17@@registry.npmjs.org@@@1")); err != nil {
			t.Errorf("the link to the version installed taken out: %v", err)
		}
	}
}
