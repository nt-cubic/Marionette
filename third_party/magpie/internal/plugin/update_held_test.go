package plugin

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha1"
	"crypto/sha512"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// npmTarball is an npm package's tarball: package/package.json and an
// index.js the host can load.
func npmTarball(t *testing.T, name, version string) []byte {
	t.Helper()
	var buf bytes.Buffer
	gz := gzip.NewWriter(&buf)
	tw := tar.NewWriter(gz)
	pj, _ := json.Marshal(map[string]string{"name": name, "version": version, "main": "index.js", "type": "module"})
	for _, f := range []struct {
		name string
		body []byte
	}{{"package/package.json", pj}, {"package/index.js", []byte("export default async () => ({})\n")}} {
		if err := tw.WriteHeader(&tar.Header{Name: f.name, Mode: 0o644, Size: int64(len(f.body)), ModTime: time.Unix(1700000000, 0)}); err != nil {
			t.Fatal(err)
		}
		tw.Write(f.body)
	}
	tw.Close()
	gz.Close()
	return buf.Bytes()
}

// A registry of the package's versions, published at the times given,
// npm's newest last: what bun and magpie both ask.
func heldRegistry(t *testing.T, pkg string, published map[string]time.Time, newest string) {
	t.Helper()
	tarballs := map[string][]byte{}
	var base string
	fakeNPM(t, func(w http.ResponseWriter, r *http.Request) {
		p, _ := url.PathUnescape(r.URL.Path)
		if strings.HasPrefix(p, "/downloads/") {
			w.Write([]byte(`{"downloads":1}`))
			return
		}
		if strings.HasPrefix(p, "/tarballs/") {
			w.Write(tarballs[strings.TrimPrefix(p, "/tarballs/")])
			return
		}
		if p == "/"+pkg+"/latest" {
			json.NewEncoder(w).Encode(map[string]string{"name": pkg, "version": newest})
			return
		}
		if p != "/"+pkg {
			http.NotFound(w, r)
			return
		}
		versions, times := map[string]any{}, map[string]string{}
		for v, at := range published {
			tgz := tarballs[v]
			s512, s1 := sha512.Sum512(tgz), sha1.Sum(tgz)
			versions[v] = map[string]any{"name": pkg, "version": v, "main": "index.js", "dist": map[string]string{
				"tarball":   base + "/tarballs/" + v,
				"integrity": "sha512-" + base64.StdEncoding.EncodeToString(s512[:]),
				"shasum":    hex.EncodeToString(s1[:]),
			}}
			times[v] = at.UTC().Format(time.RFC3339Nano)
		}
		times["modified"] = published[newest].UTC().Format(time.RFC3339Nano)
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]any{"name": pkg, "dist-tags": map[string]string{"latest": newest}, "versions": versions, "time": times})
	})
	base = npmRegistry
	for v := range published {
		tarballs[v] = npmTarball(t, pkg, v)
	}
	t.Setenv("BUN_CONFIG_REGISTRY", base+"/")
}

// An update bun won't install says why, and nothing says it was made
// (sweanng424 on Discord: Factory stayed at 0.1.18 with 0.1.21 out, through
// restarts and the hourly updates). A minimumReleaseAge in the user's
// bunfig made bun take the newest version older than it for "latest", and
// exit 0: magpie's Update, Update all, the hourly update and the moved
// built-in's update each said nothing, or that it was updated.
func TestUpdateBunHoldsBackSaysWhy(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-factory-auth"
	heldRegistry(t, pkg, map[string]time.Time{"0.1.18": time.Now().Add(-72 * time.Hour), "0.1.21": time.Now().Add(-time.Hour)}, "0.1.21")
	// the dist branch has 0.1.21 too: the user's hold is kept, not stepped round
	fakeDist(t, map[string][]string{pkg: {"0.1.18", "0.1.21"}}, false)
	home, _ := os.UserHomeDir()
	bunfig := []byte("[install]\nminimumReleaseAge = 172800\n")
	for _, p := range []string{filepath.Join(home, ".bunfig.toml"), filepath.Join(os.Getenv("XDG_CONFIG_HOME"), ".bunfig.toml")} {
		os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, bunfig, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()

	// installed when 0.1.18 was the newest, unpinned, as a move installs it
	if _, err := Add(ctx, pkg+"@0.1.18"); err != nil {
		t.Fatal(err)
	}
	if err := save(List{Plugins: []Entry{{Spec: pkg + "@latest"}}}); err != nil {
		t.Fatal(err)
	}
	// what bun does with "latest" here: the older version, and no error
	if err := install(ctx, pkg+"@latest"); err != nil {
		t.Fatal(err)
	}
	if v := Installed(pkg); v != "0.1.18" {
		t.Skipf("bun took %q for latest: it didn't read the bunfig in %s", v, home)
	}

	err := Upgrade(ctx, pkg)
	if err == nil || !strings.HasPrefix(err.Error(), pkg+" 0.1.21 isn't installed yet: Bun's minimumReleaseAge (in a .bunfig.toml) holds back versions published less than 2 days ago") || !strings.Contains(err.Error(), "blocked by minimum-release-age") {
		t.Fatalf("Update with 0.1.21 held back by Bun: %v, want bun's reason (installed %s)", err, Installed(pkg))
	}
	if err := Update(ctx); err == nil || !strings.Contains(err.Error(), "minimum-release-age") {
		t.Fatalf("Update all: %v, want bun's reason", err)
	}
	u, err := CheckUpdates(ctx)
	if err == nil || !strings.Contains(err.Error(), "minimum-release-age") {
		t.Fatalf("the hourly update: %v, want bun's reason", err)
	}
	if len(u.Updated) != 0 {
		t.Fatalf("the hourly update said it updated: %+v", u.Updated)
	}
	if v := Installed(pkg); v != "0.1.18" {
		t.Fatalf("installed %s", v)
	}

	// once bun takes it, Update installs it, and the plugin stays unpinned
	for _, p := range []string{filepath.Join(home, ".bunfig.toml"), filepath.Join(os.Getenv("XDG_CONFIG_HOME"), ".bunfig.toml")} {
		os.Remove(p)
	}
	if err := Upgrade(ctx, pkg); err != nil {
		t.Fatal(err)
	}
	if v := Installed(pkg); v != "0.1.21" {
		t.Fatalf("installed %s after the update, want 0.1.21", v)
	}
	if l := Load().Plugins; len(l) != 1 || l[0].Spec != pkg+"@latest" {
		t.Fatalf("plugins = %+v, want %s@latest", l, pkg)
	}
}

func TestAgeText(t *testing.T) {
	for in, want := range map[string]string{"172800": "2 days", "86400": "1 day", "43200": "12 hours", "600": "10 minutes", "90": "90 seconds", "x": "x seconds"} {
		if got := ageText(in); got != want {
			t.Errorf("ageText(%s) = %q, want %q", in, got, want)
		}
	}
}

// Update all (and magpie plugin update) brings a plugin pinned to an older
// version to npm's newest, unpinned, as its row's Update does: the page
// counts it among the updates, and before, Update all installed the pinned
// version again, said "Plugins updated", and the row still said the newer
// one was out (ARNO on Discord: 非官方插件…更新到最新版，magpie依然提示有更新).
// A pinned one already at npm's newest stays pinned.
func TestUpdateAllUpdatesAPinnedPlugin(t *testing.T) {
	sandbox(t)
	const pkg = "opencode-someones-auth"
	heldRegistry(t, pkg, map[string]time.Time{"1.0.0": time.Now().Add(-72 * time.Hour), "2.0.0": time.Now().Add(-48 * time.Hour)}, "2.0.0")
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()

	if _, err := Add(ctx, pkg+"@1.0.0"); err != nil {
		t.Fatal(err)
	}
	if !Pinned(Load().Plugins[0].Spec) || Installed(pkg) != "1.0.0" {
		t.Fatalf("added %+v at %s, want it pinned at 1.0.0", Load().Plugins, Installed(pkg))
	}
	if err := Prefer(pkg, "someone"); err != nil {
		t.Fatal(err)
	}
	if _, err := CheckUpdates(ctx); err != nil {
		t.Fatal(err)
	}
	if w := PendingUpdates().Waiting; len(w) != 1 || w[0].Latest != "2.0.0" {
		t.Fatalf("waiting before Update all = %+v, want 2.0.0", w)
	}

	if err := Update(ctx); err != nil {
		t.Fatal(err)
	}
	if v := Installed(pkg); v != "2.0.0" {
		t.Fatalf("Update all left %s at %s, want 2.0.0", pkg, v)
	}
	l := Load()
	if len(l.Plugins) != 1 || l.Plugins[0].Spec != pkg+"@latest" {
		t.Fatalf("plugins after Update all = %+v, want %s@latest", l.Plugins, pkg)
	}
	if l.Prefer["someone"] != pkg+"@latest" {
		t.Fatalf("the provider picked for it = %q, want it kept on %s@latest", l.Prefer["someone"], pkg)
	}
	if w := PendingUpdates().Waiting; len(w) != 0 {
		t.Fatalf("still waiting after Update all: %+v", w)
	}
	if _, err := CheckUpdates(ctx); err != nil {
		t.Fatal(err)
	}
	if w := PendingUpdates().Waiting; len(w) != 0 {
		t.Fatalf("the next look found an update again: %+v", w)
	}

	// pinned at the newest: Update all leaves its pin
	if _, err := Add(ctx, pkg+"@2.0.0"); err != nil {
		t.Fatal(err)
	}
	if err := Update(ctx); err != nil {
		t.Fatal(err)
	}
	if l := Load().Plugins; len(l) != 1 || l[0].Spec != pkg+"@2.0.0" || Installed(pkg) != "2.0.0" {
		t.Fatalf("plugins = %+v at %s, want %s@2.0.0 kept", l, Installed(pkg), pkg)
	}
}
