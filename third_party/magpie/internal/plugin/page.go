package plugin

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
)

// WindowSpec is what a plugin adds to magpie's window: its page
// (package.json's magpie.page), its panels in magpie's own pages
// (magpie.panels), the items it adds to magpie's menus (magpie.actions),
// and the reads magpie.permissions asks for, which its page and its
// panels share.
type WindowSpec struct {
	Page   *PageSpec // nil when it adds no page of its own
	Panels []PanelSpec
	// Actions are its items in magpie's menus, each opening its page;
	// none without a page
	Actions []ActionSpec
	// Reads are the reads it asked for that magpie has (PageReads), in
	// the order asked.
	Reads []string
}

// PageSpec is a plugin's own page in magpie's window.
type PageSpec struct {
	Title string // "" when it names none
	// Titles are its title in other languages, by the window's language
	// ("zh", "zh-TW", "ja", "de"): magpie.page.titles
	Titles map[string]string
	Icon   string // a file in the page's folder, relative to it, or ""
	File   string // the page's HTML, inside the plugin's folder
	Dir    string // the folder the page's files are served from: File's
	// Replaces is the built-in page (one of Replaceable) whose place in
	// the sidebar the page takes while its plugin is on, or "".
	Replaces string
}

// PanelSpec is a panel a plugin adds to one of magpie's own pages: a
// part of that page, drawn by the plugin's HTML in a frame of its own.
type PanelSpec struct {
	Slot   string // one of PanelSlots
	Title  string // "" when it names none
	Titles map[string]string
	File   string // the panel's HTML, inside the plugin's folder
	Dir    string // the folder the panel's files are served from: File's
}

// ActionSpec is an item a plugin adds to one of magpie's menus
// (ActionMenus). Picking it opens the plugin's page, which is told the
// action's id and the row it was picked on (magpie.onAction).
type ActionSpec struct {
	ID     string // its id among the plugin's actions: actionID
	Menu   string // one of ActionMenus
	Title  string // "" when it names none: the page's title then
	Titles map[string]string
}

// ActionMenus are the menus a plugin's action can be in: a provider
// row's (the page is told the provider's id and name) and an agent row's
// (the agent's id and name). Nothing else of the row is told.
var ActionMenus = []string{"provider", "agent"}

// actionID is what an action's id is: short, of lower-case letters,
// digits and dashes, so the page can switch on it
var actionID = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,39}$`)

// maxActions is how many actions one plugin adds, at most.
const maxActions = 8

// PageReads are the reads a plugin's page can be granted, each in
// magpie.permissions. None of them carries a secret.
var PageReads = []string{"usage:summary", "providers:list"}

// PanelSlots are the places in magpie's own pages a plugin's panel can
// go: under the Agents page's list, at the end of the Usage page, on
// Settings' Plugins tab, and in a provider's editor (which tells the
// panel the provider's id and name).
var PanelSlots = []string{"agents", "usage", "settings", "provider"}

// Replaceable are the built-in pages a plugin's page can take the place
// of: the pages that are built-in plugins of their own.
var Replaceable = []string{"sessions", "library", "routing", "usage"}

// titles are a page's or a panel's titles by language, without the empty
// ones, or nil: magpie.page.titles {"zh": "…"}. title stays a string
// beside them, so a magpie that knows no titles still reads the package,
// and titles that aren't an object of strings leave the page as it was.
func titles(raw json.RawMessage) map[string]string {
	var m map[string]any
	if json.Unmarshal(raw, &m) != nil {
		return nil
	}
	var out map[string]string
	for k, x := range m {
		v, _ := x.(string)
		k, v = strings.TrimSpace(k), strings.TrimSpace(v)
		if k == "" || v == "" || len(k) > 16 || len(out) == 16 {
			continue
		}
		if out == nil {
			out = map[string]string{}
		}
		out[k] = v
	}
	return out
}

// maxPanels is how many panels one plugin adds, at most.
const maxPanels = 8

// Window is what a plugin at target adds to magpie's window, or nil when
// it adds nothing: package.json's magpie.page {title, file, icon,
// replaces}, magpie.panels [{slot, file, title}] and magpie.actions [{id,
// menu, title}]. A file outside the
// plugin's folder, or not .html, is no page or panel; a panel in a slot
// magpie doesn't have is left out, and so is a replaces of a page that
// can't be replaced. only is whether it is nothing else: no OpenCode
// plugin for the host to load (no main, no exports).
func Window(target string) (w *WindowSpec, only bool) {
	st, err := os.Stat(target)
	if err != nil || !st.IsDir() {
		return nil, false
	}
	b, err := os.ReadFile(filepath.Join(target, "package.json"))
	if err != nil {
		return nil, false
	}
	type part struct {
		Title    string          `json:"title"`
		Titles   json.RawMessage `json:"titles"`
		File     string          `json:"file"`
		Icon     string          `json:"icon"`
		Slot     string          `json:"slot"`
		Replaces string          `json:"replaces"`
		ID       string          `json:"id"`
		Menu     string          `json:"menu"`
	}
	var pkg struct {
		Main    string          `json:"main"`
		Exports json.RawMessage `json:"exports"`
		Magpie  struct {
			Page        *part    `json:"page"`
			Panels      []part   `json:"panels"`
			Actions     []part   `json:"actions"`
			Permissions []string `json:"permissions"`
		} `json:"magpie"`
	}
	if json.Unmarshal(b, &pkg) != nil {
		return nil, false
	}
	w = &WindowSpec{}
	if pg := pkg.Magpie.Page; pg != nil {
		if file, ok := htmlInside(target, pg.File); ok {
			p := &PageSpec{Title: strings.TrimSpace(pg.Title), Titles: titles(pg.Titles), File: file, Dir: filepath.Dir(file)}
			// the icon is served with the page, so it sits in the page's folder
			if icon, ok := inside(target, pg.Icon); ok {
				if rel, err := filepath.Rel(p.Dir, icon); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
					p.Icon = filepath.ToSlash(rel)
				}
			}
			if r := strings.TrimSpace(pg.Replaces); slices.Contains(Replaceable, r) {
				p.Replaces = r
			}
			w.Page = p
		}
	}
	for _, pn := range pkg.Magpie.Panels {
		if len(w.Panels) == maxPanels {
			break
		}
		slot := strings.TrimSpace(pn.Slot)
		file, ok := htmlInside(target, pn.File)
		if !ok || !slices.Contains(PanelSlots, slot) {
			continue
		}
		w.Panels = append(w.Panels, PanelSpec{Slot: slot, Title: strings.TrimSpace(pn.Title), Titles: titles(pn.Titles), File: file, Dir: filepath.Dir(file)})
	}
	// an action opens the plugin's page, so a plugin with none has no
	// action; an id given twice is the first's
	for _, a := range pkg.Magpie.Actions {
		if w.Page == nil || len(w.Actions) == maxActions {
			break
		}
		id, menu := strings.TrimSpace(a.ID), strings.TrimSpace(a.Menu)
		if !actionID.MatchString(id) || !slices.Contains(ActionMenus, menu) || slices.ContainsFunc(w.Actions, func(x ActionSpec) bool { return x.ID == id }) {
			continue
		}
		w.Actions = append(w.Actions, ActionSpec{ID: id, Menu: menu, Title: strings.TrimSpace(a.Title), Titles: titles(a.Titles)})
	}
	if w.Page == nil && len(w.Panels) == 0 {
		return nil, false
	}
	for _, r := range pkg.Magpie.Permissions {
		if slices.Contains(PageReads, r) && !slices.Contains(w.Reads, r) {
			w.Reads = append(w.Reads, r)
		}
	}
	return w, pkg.Main == "" && len(pkg.Exports) == 0
}

// htmlInside is rel joined to dir, if that stays inside dir and is an
// .html file.
func htmlInside(dir, rel string) (string, bool) {
	file, ok := inside(dir, rel)
	if !ok || !strings.HasSuffix(strings.ToLower(file), ".html") {
		return "", false
	}
	if st, err := os.Stat(file); err != nil || st.IsDir() {
		return "", false
	}
	return file, true
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

// FileIn is the file at rel among the files served from dir (a page's
// or a panel's folder), if it is one: inside dir, and a file.
func FileIn(dir, rel string) (string, bool) {
	f, ok := inside(dir, rel)
	if !ok {
		return "", false
	}
	if st, err := os.Stat(f); err != nil || st.IsDir() {
		return "", false
	}
	return f, true
}

// SetPageOptions sets the options of a plugin from its page or a panel.
// One whose page and panels are all it has is never loaded by the host, so the host goes on as
// it was; any other is SetOptions, which starts the host again for it.
func SetPageOptions(spec string, opts map[string]any) error {
	t := Target(spec)
	if mw, _ := Middleware(t); mw != "" {
		return SetOptions(spec, opts)
	}
	if ag, _ := Agent(t); ag != "" {
		return SetOptions(spec, opts)
	}
	if w, only := Window(t); w == nil || !only {
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
