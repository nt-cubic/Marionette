package plugin

import (
	"context"
	"crypto/sha512"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakeDist serves the community repo's dist branch, as scripts/dist.mjs
// there writes it, with the versions given of each package (the last one
// named its latest); tamper serves every tarball other than the one its
// index names.
func fakeDist(t *testing.T, pkgs map[string][]string, tamper bool) {
	t.Helper()
	files := map[string][]byte{}
	idx := map[string]any{}
	for name, versions := range pkgs {
		vs := map[string]any{}
		for _, v := range versions {
			tgz := npmTarball(t, name, v)
			file := strings.ReplaceAll(strings.TrimPrefix(name, "@"), "/", "-") + "-" + v + ".tgz"
			sum := sha512.Sum512(tgz)
			if tamper {
				tgz = npmTarball(t, name, v+"-not")
			}
			files["/tgz/"+file] = tgz
			pj, _ := json.Marshal(map[string]string{"name": name, "version": v, "description": "from dist", "author": "dbydd"})
			vs[v] = map[string]any{"file": "tgz/" + file, "integrity": "sha512-" + base64.StdEncoding.EncodeToString(sum[:]), "manifest": json.RawMessage(pj)}
		}
		idx[name] = map[string]any{"latest": versions[len(versions)-1], "readme": "# " + name + " from dist", "versions": vs}
	}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/index.json" {
			json.NewEncoder(w).Encode(map[string]any{"version": 1, "packages": idx})
			return
		}
		if b, ok := files[r.URL.Path]; ok {
			w.Write(b)
			return
		}
		http.NotFound(w, r)
	}))
	t.Cleanup(srv.Close)
	forgetDist := func() {
		distMu.Lock()
		distIdx, distAt, distOK = nil, time.Time{}, false
		distMu.Unlock()
	}
	base := distBase
	distBase = srv.URL
	forgetDist()
	t.Cleanup(func() { distBase = base; forgetDist() })
}

// npmHasNothing is an npm registry that has no package at all: a new
// plugin, merged before anyone could publish it there.
func npmHasNothing(t *testing.T) {
	fakeNPM(t, func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/downloads/") {
			w.Write([]byte(`{"downloads":0}`))
			return
		}
		http.NotFound(w, r)
	})
	t.Setenv("BUN_CONFIG_REGISTRY", npmRegistry+"/")
}

// A plugin merged into the community repo reaches magpie before npm has
// it: its first npm version waits for an owner's 2FA, which kept reviewed
// plugins unmerged (magpie-community/plugins#84, Open Design). The market
// shows its version and README, and adding it installs the dist branch's
// tarball with real bun.
func TestDistInstallsAPackageNPMDoesntHave(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-opendesign-auth"
	npmHasNothing(t)
	fakeDist(t, map[string][]string{pkg: {"0.1.3", "0.1.4"}}, false)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	if info := Info(ctx, []string{pkg})[pkg]; info.Version != "0.1.4" || info.Description != "from dist" || info.Publisher != "dbydd" {
		t.Fatalf("the market says %+v, want the dist branch's 0.1.4", info)
	}
	if p, err := Readme(ctx, pkg); err != nil || !strings.Contains(p.Readme, "from dist") {
		t.Fatalf("README: %+v, %v", p, err)
	}
	if _, err := Add(ctx, pkg); err != nil {
		t.Fatalf("Add: %v", err)
	}
	if v := Installed(pkg); v != "0.1.4" {
		t.Fatalf("installed %q, want 0.1.4", v)
	}
	if _, err := os.Stat(filepath.Join(Dir(), "dist", "magpie-community-opencode-opendesign-auth-0.1.4.tgz")); err != nil {
		t.Fatalf("the tarball bun.lock names isn't kept: %v", err)
	}
	// a pinned, older version too, and the newer one's tarball goes
	if err := installAt(ctx, pkg, "0.1.3"); err != nil {
		t.Fatalf("installAt 0.1.3: %v", err)
	}
	if v := Installed(pkg); v != "0.1.3" {
		t.Fatalf("installed %q, want 0.1.3", v)
	}
	if left, _ := filepath.Glob(filepath.Join(Dir(), "dist", "*.tgz")); len(left) != 1 || !strings.HasSuffix(left[0], "-0.1.3.tgz") {
		t.Fatalf("plugins/dist holds %v, want 0.1.3's tarball alone", left)
	}
}

// A version npm refused (its Trusted Publisher allowing stage publish
// only, as zed 0.1.7 and zen-free 0.1.7 were) comes from the dist branch,
// and once npm has a newer one, from npm again, the tarball taken out.
func TestDistServesAVersionNPMRefused(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-zed-auth"
	old := time.Now().Add(-72 * time.Hour)
	heldRegistry(t, pkg, map[string]time.Time{"0.1.6": old}, "0.1.6")
	fakeDist(t, map[string][]string{pkg: {"0.1.6", "0.1.7"}}, false)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	if _, err := Add(ctx, pkg+"@0.1.6"); err != nil {
		t.Fatal(err)
	}
	if err := save(List{Plugins: []Entry{{Spec: pkg + "@latest"}}}); err != nil {
		t.Fatal(err)
	}
	if err := Upgrade(ctx, pkg); err != nil {
		t.Fatalf("Upgrade to the dist branch's 0.1.7: %v", err)
	}
	if v := Installed(pkg); v != "0.1.7" {
		t.Fatalf("installed %q, want 0.1.7", v)
	}

	heldRegistry(t, pkg, map[string]time.Time{"0.1.6": old, "0.1.8": old}, "0.1.8")
	fakeDist(t, map[string][]string{pkg: {"0.1.6", "0.1.7", "0.1.8"}}, false)
	if err := Upgrade(ctx, pkg); err != nil {
		t.Fatalf("Upgrade to npm's 0.1.8: %v", err)
	}
	if v := Installed(pkg); v != "0.1.8" {
		t.Fatalf("installed %q, want 0.1.8", v)
	}
	b, _ := os.ReadFile(filepath.Join(Dir(), "package.json"))
	if !strings.Contains(string(b), `"`+pkg+`": "0.1.8"`) {
		t.Fatalf("plugins/package.json doesn't name npm's 0.1.8:\n%s", b)
	}
	if left, _ := filepath.Glob(filepath.Join(Dir(), "dist", "*.tgz")); len(left) != 0 {
		t.Fatalf("plugins/dist still holds %v", left)
	}
}

// A tarball that isn't the one the index names is never installed.
func TestDistRefusesATarballNotItsIndexs(t *testing.T) {
	sandbox(t)
	const pkg = "@magpie-community/opencode-opendesign-auth"
	npmHasNothing(t)
	fakeDist(t, map[string][]string{pkg: {"0.1.4"}}, true)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Minute)
	defer cancel()

	_, err := Add(ctx, pkg)
	if err == nil || !strings.Contains(err.Error(), "isn't the tarball its index names") {
		t.Fatalf("Add: %v, want the sha512 refused", err)
	}
	if v := Installed(pkg); v != "" {
		t.Fatalf("installed %q", v)
	}
	if left, _ := filepath.Glob(filepath.Join(Dir(), "dist", "*")); len(left) != 0 {
		t.Fatalf("plugins/dist holds %v", left)
	}
}

func TestDistWanted(t *testing.T) {
	for spec, want := range map[string]string{
		"@magpie-community/opencode-zed-auth":           "@magpie-community/opencode-zed-auth latest",
		"@magpie-community/opencode-zed-auth@latest":    "@magpie-community/opencode-zed-auth latest",
		"@magpie-community/opencode-zed-auth@0.1.7":     "@magpie-community/opencode-zed-auth 0.1.7",
		"@magpie-community/opencode-zed-auth@^0.1.7":    "",
		"@magpie-community/opencode-zed-auth@next":      "",
		"opencode-zed-auth@0.1.7":                       "",
		"@someone/opencode-zed-auth@0.1.7":              "",
		"github:magpie-community/plugins":               "",
		"./dist/magpie-community-opencode-zed-auth.tgz": "",
	} {
		name, version, ok := distWanted(spec)
		got := ""
		if ok {
			got = name + " " + version
		}
		if got != want {
			t.Errorf("distWanted(%q) = %q, want %q", spec, got, want)
		}
	}
	for file, ok := range map[string]bool{"tgz/a-0.1.0.tgz": true, "tgz/../a.tgz": false, "../a.tgz": false, "tgz/a/b.tgz": false, "tgz/a.zip": false} {
		if distFile.MatchString(file) != ok {
			t.Errorf("distFile %q: %v, want %v", file, !ok, ok)
		}
	}
}
