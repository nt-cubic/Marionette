package plugin

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// PageSpec is the page a plugin adds to magpie's window: package.json's
// magpie.page, and the reads magpie.permissions asks for.
type PageSpec struct {
	Title string // "" when it names none
	Icon  string // a file in the page's folder, relative to it, or ""
	File  string // the page's HTML, inside the plugin's folder
	Dir   string // the folder the page's files are served from: File's
	// Reads are the reads the page asked for that magpie has (PageReads),
	// in the order asked.
	Reads []string
}

// PageReads are the reads a plugin's page can be granted, each in
// magpie.permissions. None of them carries a secret.
var PageReads = []string{"usage:summary", "providers:list"}

// WindowPage is the page a plugin at target adds to magpie's window, or nil:
// package.json's magpie.page {title, file, icon}. A file outside the
// plugin's folder is no page. only is whether it is nothing else: no
// OpenCode plugin for the host to load (no main, no exports).
func WindowPage(target string) (p *PageSpec, only bool) {
	st, err := os.Stat(target)
	if err != nil || !st.IsDir() {
		return nil, false
	}
	b, err := os.ReadFile(filepath.Join(target, "package.json"))
	if err != nil {
		return nil, false
	}
	var pkg struct {
		Main    string          `json:"main"`
		Exports json.RawMessage `json:"exports"`
		Magpie  struct {
			Page *struct {
				Title string `json:"title"`
				File  string `json:"file"`
				Icon  string `json:"icon"`
			} `json:"page"`
			Permissions []string `json:"permissions"`
		} `json:"magpie"`
	}
	if json.Unmarshal(b, &pkg) != nil || pkg.Magpie.Page == nil {
		return nil, false
	}
	pg := pkg.Magpie.Page
	file, ok := inside(target, pg.File)
	if !ok || !strings.HasSuffix(strings.ToLower(file), ".html") {
		return nil, false
	}
	p = &PageSpec{Title: strings.TrimSpace(pg.Title), File: file, Dir: filepath.Dir(file)}
	// the icon is served with the page, so it sits in the page's folder
	if icon, ok := inside(target, pg.Icon); ok {
		if rel, err := filepath.Rel(p.Dir, icon); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			p.Icon = filepath.ToSlash(rel)
		}
	}
	for _, r := range pkg.Magpie.Permissions {
		if slices.Contains(PageReads, r) && !slices.Contains(p.Reads, r) {
			p.Reads = append(p.Reads, r)
		}
	}
	return p, pkg.Main == "" && len(pkg.Exports) == 0
}

// inside is rel joined to dir, if that stays inside dir: no "..", no
// absolute path, and no symbolic link out of it.
func inside(dir, rel string) (string, bool) {
	rel = strings.TrimSpace(rel)
	if rel == "" || filepath.IsAbs(rel) || strings.HasPrefix(rel, "/") || strings.Contains(rel, `\`) {
		return "", false
	}
	clean := filepath.Clean(filepath.FromSlash(rel))
	if clean == ".." || strings.HasPrefix(clean, ".."+string(filepath.Separator)) {
		return "", false
	}
	full := filepath.Join(dir, clean)
	realDir, err := filepath.EvalSymlinks(dir)
	if err != nil {
		return "", false
	}
	real, err := filepath.EvalSymlinks(full)
	if err != nil {
		return "", false
	}
	if r, err := filepath.Rel(realDir, real); err != nil || r == ".." || strings.HasPrefix(r, ".."+string(filepath.Separator)) {
		return "", false
	}
	return full, true
}

// PageFile is the file at rel among a page's files, if it is one: inside
// the page's folder, and a file.
func (p *PageSpec) PageFile(rel string) (string, bool) {
	f, ok := inside(p.Dir, rel)
	if !ok {
		return "", false
	}
	if st, err := os.Stat(f); err != nil || st.IsDir() {
		return "", false
	}
	return f, true
}

// SetPageOptions sets the options of a plugin from its page. One whose
// page is all it has is never loaded by the host, so the host goes on as
// it was; any other is SetOptions, which starts the host again for it.
func SetPageOptions(spec string, opts map[string]any) error {
	t := Target(spec)
	if mw, _ := Middleware(t); mw != "" {
		return SetOptions(spec, opts)
	}
	if ag, _ := Agent(t); ag != "" {
		return SetOptions(spec, opts)
	}
	if p, only := WindowPage(t); p == nil || !only {
		return SetOptions(spec, opts)
	}
	listMu.Lock()
	defer listMu.Unlock()
	l := Load()
	i := indexOf(l, spec)
	if i < 0 {
		return fmt.Errorf("no plugin %q", spec)
	}
	if len(opts) == 0 {
		opts = nil
	}
	l.Plugins[i].Options = opts
	return save(l)
}
