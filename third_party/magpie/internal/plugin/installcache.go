package plugin

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"

	"github.com/yetone/magpie/internal/catalog"
)

// installCacheDir is Bun's install cache for plugins/: each version of a
// package it has fetched, unpacked.
func installCacheDir() string {
	return filepath.Join(filepath.Dir(catalog.CachePath()), "bun", "install-cache")
}

// pruneInstallCache takes out of Bun's install cache the versions
// plugins/ no longer has installed. Bun never does: every version of every
// plugin an update ever fetched stayed, 131 of them and growing by one
// with each release of a moved subscription's plugin (#1988). What is
// installed is a copy, so a version taken out is only fetched again if it
// is asked for again. installMu held.
func pruneInstallCache() {
	root := installCacheDir()
	tops, err := os.ReadDir(root)
	if err != nil {
		return
	}
	pruneCached(root, "", tops)
	for _, top := range tops {
		if top.IsDir() && strings.HasPrefix(top.Name(), "@") && !strings.Contains(top.Name(), "@@") {
			// a scope: its packages are in it
			dir := filepath.Join(root, top.Name())
			if ents, err := os.ReadDir(dir); err == nil {
				pruneCached(dir, top.Name()+"/", ents)
			}
		}
	}
}

// pruneCached takes out of ents, dir's entries in the install cache, the
// versions (name@version@@registry…) plugins/ hasn't installed; then, in
// each package's folder of links to its versions, the links left to
// nothing.
func pruneCached(dir, scope string, ents []os.DirEntry) {
	var folders []string
	for _, e := range ents {
		if !e.IsDir() {
			continue
		}
		name, rest, ok := strings.Cut(e.Name(), "@")
		if !ok {
			folders = append(folders, filepath.Join(dir, e.Name()))
			continue
		}
		version, _, ok := strings.Cut(rest, "@@")
		if !ok || name == "" || version == "" {
			continue
		}
		if v, known := installedVersion(scope + name); known && v != version {
			os.RemoveAll(filepath.Join(dir, e.Name()))
		}
	}
	for _, f := range folders {
		links, _ := os.ReadDir(f)
		for _, l := range links {
			p := filepath.Join(f, l.Name())
			if l.Type()&os.ModeSymlink == 0 {
				continue
			}
			if _, err := os.Stat(p); os.IsNotExist(err) {
				os.Remove(p)
			}
		}
	}
}

// installedVersion is the version of pkg in plugins/node_modules, "" when
// it isn't there. known is false when that can't be told (a read or parse
// that failed): its versions stay then.
func installedVersion(pkg string) (version string, known bool) {
	b, err := os.ReadFile(filepath.Join(Dir(), "node_modules", filepath.FromSlash(pkg), "package.json"))
	if os.IsNotExist(err) {
		return "", true
	}
	if err != nil {
		return "", false
	}
	var pj struct {
		Version string `json:"version"`
	}
	if json.Unmarshal(b, &pj) != nil || pj.Version == "" {
		return "", false
	}
	return pj.Version, true
}
