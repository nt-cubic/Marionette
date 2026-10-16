package plugin

import (
	"context"
	"crypto/sha512"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"
)

// The community repo's dist branch: every version of each
// @magpie-community package, packed by the repo's publish workflow
// (scripts/dist.mjs there) as soon as it is merged. npm can't take a new
// package's first version without an owner's 2FA (trusted publishing sets
// up only a package that exists), so a reviewed plugin used to wait,
// unmerged, for someone to publish it by hand. magpie asks npm first, as
// before, and turns here only for what npm doesn't give: a package or
// version npm doesn't have, or npm not answering. A tarball is installed
// only when its sha512 is the one index.json names.

// distBase is where the branch is read; moved by tests. Read through the
// 「国内镜像」 switch's jsDelivr copy first when it is on (source.DoFaithful).
var distBase = "https://raw.githubusercontent.com/magpie-community/plugins/dist"

// distScope is the scope of the packages the branch has.
const distScope = "@magpie-community/"

type distVersion struct {
	File      string          `json:"file"`      // tgz/<npm pack's file name>
	Integrity string          `json:"integrity"` // sha512-<base64>, as npm writes it
	Manifest  json.RawMessage `json:"manifest"`  // the version's package.json
}

type distPackage struct {
	Latest   string                 `json:"latest"`
	Readme   string                 `json:"readme"`
	Versions map[string]distVersion `json:"versions"`
}

type distIndex struct {
	Packages map[string]distPackage `json:"packages"`
}

var (
	distMu   sync.Mutex
	distIdx  *distIndex
	distAt   time.Time // when distIdx was read, or last failed to be
	distOK   bool      // whether the last read was distIdx's
	distKeep = 10 * time.Minute
	distWait = time.Minute // after a read failed, before asking again
)

// distFile is a file the index may name: one tarball under tgz/, nothing
// that leaves the branch or plugins/dist.
var distFile = regexp.MustCompile(`^tgz/[A-Za-z0-9._-]+\.tgz$`)

// distPkg is what the dist branch has of the package, nil when it has
// none, the package isn't one of its scope, or the branch can't be read.
func distPkg(ctx context.Context, name string) *distPackage {
	if !strings.HasPrefix(name, distScope) || distBase == "" {
		return nil
	}
	distMu.Lock()
	defer distMu.Unlock()
	stale := distIdx == nil || time.Since(distAt) > distKeep
	if stale && (distOK || distAt.IsZero() || time.Since(distAt) > distWait) {
		b, err := fetchJSONFaithful(ctx, distBase+"/index.json", 32<<20)
		var idx distIndex
		if err == nil && json.Unmarshal(b, &idx) == nil {
			distIdx, distOK = &idx, true
		} else {
			// what was read last stays, until a read works
			distOK = false
		}
		distAt = time.Now()
	}
	if distIdx == nil {
		return nil
	}
	p, ok := distIdx.Packages[name]
	if !ok {
		return nil
	}
	return &p
}

// distInfo is the package's newest version on the dist branch, as npm's
// /latest would say it, and whether there is one.
func distInfo(ctx context.Context, name string) (NPM, bool) {
	d := distPkg(ctx, name)
	if d == nil {
		return NPM{}, false
	}
	v, ok := d.Versions[d.Latest]
	if !ok {
		return NPM{}, false
	}
	var l npmLatest
	if json.Unmarshal(v.Manifest, &l) != nil {
		return NPM{}, false
	}
	info := npmOf(l)
	info.Version = d.Latest
	return info, true
}

// distWanted is the package and version an npm spec asks for, when the
// dist branch could serve it: a @magpie-community package, at a version
// or "latest" (not a range or another tag).
func distWanted(spec string) (name, version string, ok bool) {
	if IsPath(spec) || IsGit(spec) {
		return "", "", false
	}
	name = Name(spec)
	if !strings.HasPrefix(name, distScope) || !pkgName.MatchString(name) {
		return "", "", false
	}
	version = strings.TrimPrefix(strings.TrimPrefix(spec, name), "@")
	if version == "" {
		version = "latest"
	}
	if version != "latest" && !exactVersion.MatchString(version) {
		return "", "", false
	}
	return name, version, true
}

var exactVersion = regexp.MustCompile(`^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$`)

// installDist installs the package's version from the dist branch: its
// tarball fetched, checked against the index's sha512, kept in
// plugins/dist (bun.lock names it there, and a later bun add installs it
// again from it), and added with bun. installMu held.
func installDist(ctx context.Context, bun, name, version string) error {
	d := distPkg(ctx, name)
	if d == nil {
		return fmt.Errorf("the community repo's dist branch has no %s, or can't be read", name)
	}
	if version == "latest" {
		version = d.Latest
	}
	v, ok := d.Versions[version]
	if !ok {
		return fmt.Errorf("the community repo's dist branch has no %s@%s", name, version)
	}
	if !distFile.MatchString(v.File) {
		return fmt.Errorf("the community repo's dist branch names %q for %s@%s, not a tarball of its own", v.File, name, version)
	}
	b, err := fetchJSONFaithful(ctx, distBase+"/"+v.File, 64<<20)
	if err != nil {
		return err
	}
	sum := sha512.Sum512(b)
	if got := "sha512-" + base64.StdEncoding.EncodeToString(sum[:]); got != v.Integrity {
		return fmt.Errorf("%s from the community repo's dist branch isn't the tarball its index names (sha512 %s, not %s)", v.File, got, v.Integrity)
	}
	dir := filepath.Join(Dir(), "dist")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	file := path.Base(v.File)
	tmp, err := os.CreateTemp(dir, file+".*")
	if err != nil {
		return err
	}
	_, werr := tmp.Write(b)
	if cerr := tmp.Close(); werr == nil {
		werr = cerr
	}
	if werr == nil {
		werr = os.Rename(tmp.Name(), filepath.Join(dir, file))
	}
	if werr != nil {
		os.Remove(tmp.Name())
		return werr
	}
	return bunAdd(ctx, bun, "./dist/"+file)
}

// pruneDist takes out the tarballs in plugins/dist that plugins/package.json
// no longer names: one an update from npm, or a newer one, took the place
// of. installMu held. One it can't tell about (package.json unreadable)
// stays.
func pruneDist() {
	b, err := os.ReadFile(filepath.Join(Dir(), "package.json"))
	if err != nil {
		return
	}
	var pj struct {
		Dependencies map[string]string `json:"dependencies"`
	}
	if json.Unmarshal(b, &pj) != nil {
		return
	}
	named := map[string]bool{}
	for _, s := range pj.Dependencies {
		s = strings.TrimPrefix(s, "file:")
		if rest, ok := strings.CutPrefix(filepath.ToSlash(s), "./dist/"); ok {
			named[rest] = true
		} else if rest, ok := strings.CutPrefix(filepath.ToSlash(s), "dist/"); ok {
			named[rest] = true
		}
	}
	ents, err := os.ReadDir(filepath.Join(Dir(), "dist"))
	if err != nil {
		return
	}
	for _, e := range ents {
		if !e.IsDir() && strings.HasSuffix(e.Name(), ".tgz") && !named[e.Name()] {
			os.Remove(filepath.Join(Dir(), "dist", e.Name()))
		}
	}
}
