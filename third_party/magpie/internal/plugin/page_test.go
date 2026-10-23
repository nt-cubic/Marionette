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
	w, only := Window(good)
	if w == nil || w.Page == nil || !only || w.Page.Title != "Good" || w.Page.Icon != "index.html" {
		t.Fatalf("good: %+v only=%v", w, only)
	}
	p := w.Page
	if !slices.Equal(w.Reads, []string{"usage:summary"}) {
		t.Errorf("reads %v, want only the one magpie has, once", w.Reads)
	}
	if !InMagpieOnly(good) || notPlugin(good) != nil {
		t.Errorf("a page-only plugin: InMagpieOnly %v, notPlugin %v", InMagpieOnly(good), notPlugin(good))
	}
	if _, only := Window(mk("main", `{"name":"m","main":"index.js","magpie":{"page":{"file":"page/index.html"}}}`)); only {
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
		if w, _ := Window(mk(name, pkg)); w != nil {
			t.Errorf("%s: %+v is a page", name, w)
		}
	}
	if w, _ := Window(link); w != nil {
		t.Errorf("a symbolic link out of the folder: %+v is a page", w)
	}
	if f, ok := FileIn(p.Dir, "../package.json"); ok {
		t.Errorf("FileIn served %s", f)
	}
	if f, ok := FileIn(p.Dir, "index.html"); !ok || filepath.Base(f) != "index.html" {
		t.Errorf("FileIn(index.html) = %s %v", f, ok)
	}
}

// A plugin's panels (magpie.panels) go only where magpie has a slot, each
// an .html inside the plugin's folder, at most maxPanels of them; a page
// replaces only a built-in page that can be replaced. A plugin that is
// only panels is a window plugin all the same: in magpie only, and a
// plugin.
func TestWindowPanels(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "secret.html"), []byte("<p>secret</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	d := filepath.Join(root, "p")
	if err := os.MkdirAll(filepath.Join(d, "ui"), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"ui/usage.html", "ui/prov.html", "ui/page.html"} {
		if err := os.WriteFile(filepath.Join(d, filepath.FromSlash(f)), []byte("<p>hi</p>"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write := func(pkg string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(d, "package.json"), []byte(pkg), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(`{"name":"p","magpie":{"panels":[
		{"slot":"usage","file":"ui/usage.html","title":"Spend"},
		{"slot":"provider","file":"ui/prov.html"},
		{"slot":"nowhere","file":"ui/usage.html"},
		{"slot":"usage","file":"../secret.html"},
		{"slot":"usage","file":"ui/missing.html"},
		{"slot":"agents","file":"package.json"}
	],"permissions":["providers:list"]}}`)
	w, only := Window(d)
	if w == nil || !only || w.Page != nil {
		t.Fatalf("panels only: %+v only=%v", w, only)
	}
	var got []string
	for _, p := range w.Panels {
		got = append(got, p.Slot+":"+p.Title+":"+filepath.Base(p.File))
	}
	if want := []string{"usage:Spend:usage.html", "provider::prov.html"}; !slices.Equal(got, want) {
		t.Errorf("panels %v, want %v", got, want)
	}
	if !slices.Equal(w.Reads, []string{"providers:list"}) {
		t.Errorf("reads %v", w.Reads)
	}
	if !InMagpieOnly(d) || notPlugin(d) != nil {
		t.Errorf("a panels-only plugin: InMagpieOnly %v, notPlugin %v", InMagpieOnly(d), notPlugin(d))
	}

	many := `{"slot":"usage","file":"ui/usage.html"}`
	for range maxPanels {
		many += `,{"slot":"usage","file":"ui/usage.html"}`
	}
	write(`{"name":"p","magpie":{"panels":[` + many + `]}}`)
	if w, _ := Window(d); w == nil || len(w.Panels) != maxPanels {
		t.Errorf("%+v kept, want %d panels at most", w, maxPanels)
	}

	for replaces, want := range map[string]string{"usage": "usage", "sessions": "sessions", "agents": "", "settings": "", "plugins": "", "": ""} {
		write(`{"name":"p","magpie":{"page":{"file":"ui/page.html","replaces":"` + replaces + `"}}}`)
		w, _ := Window(d)
		if w == nil || w.Page == nil || w.Page.Replaces != want {
			t.Errorf("replaces %q: %+v, want %q", replaces, w, want)
		}
	}
}

// A page's and a panel's titles by language (magpie.page.titles) are kept
// trimmed, without the empty ones; titles that aren't an object of strings
// leave the page as it was, with its title.
func TestWindowTitles(t *testing.T) {
	d := t.TempDir()
	if err := os.WriteFile(filepath.Join(d, "index.html"), []byte("<p>hi</p>"), 0o644); err != nil {
		t.Fatal(err)
	}
	write := func(titles string) *WindowSpec {
		t.Helper()
		pkg := `{"name":"p","magpie":{"page":{"title":"Glance","file":"index.html","titles":` + titles + `},
			"panels":[{"slot":"usage","file":"index.html","title":"Spend","titles":` + titles + `}]}}`
		if err := os.WriteFile(filepath.Join(d, "package.json"), []byte(pkg), 0o644); err != nil {
			t.Fatal(err)
		}
		w, _ := Window(d)
		if w == nil || w.Page == nil || len(w.Panels) != 1 || w.Page.Title != "Glance" || w.Panels[0].Title != "Spend" {
			t.Fatalf("titles %s: %+v", titles, w)
		}
		return w
	}
	w := write(`{" zh ":" 速览 ","ja":"","de":7,"":"x"}`)
	for _, m := range []map[string]string{w.Page.Titles, w.Panels[0].Titles} {
		if len(m) != 1 || m["zh"] != "速览" {
			t.Errorf("titles %v, want only zh 速览", m)
		}
	}
	for _, bad := range []string{`"速览"`, `["速览"]`, `null`} {
		if w := write(bad); w.Page.Titles != nil || w.Panels[0].Titles != nil {
			t.Errorf("titles %s: %v %v", bad, w.Page.Titles, w.Panels[0].Titles)
		}
	}
}

// A plugin's actions (magpie.actions) are items in a provider's or an
// agent's row menu that open its page: a plugin with no page has none,
// an action in a menu magpie hasn't, or with an id the page couldn't
// switch on, is dropped, an id given twice is the first's, and there are
// eight at most.
func TestWindowActions(t *testing.T) {
	root := t.TempDir()
	mk := func(name, magpie string) string {
		t.Helper()
		d := filepath.Join(root, name)
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(d, "index.html"), []byte("<p>hi</p>"), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(d, "package.json"), []byte(`{"name":"`+name+`","magpie":`+magpie+`}`), 0o644); err != nil {
			t.Fatal(err)
		}
		return d
	}
	w, _ := Window(mk("acts", `{"page":{"file":"index.html"},"actions":[
		{"id":"check","menu":"provider","title":"Check balance","titles":{"zh":"查余额"}},
		{"id":"check","menu":"agent","title":"Twice"},
		{"id":"logs","menu":"agent"},
		{"id":"Bad Id","menu":"provider","title":"x"},
		{"id":"session","menu":"session","title":"x"},
		{"id":"","menu":"provider","title":"x"}]}`))
	if w == nil {
		t.Fatal("no window")
	}
	want := []ActionSpec{
		{ID: "check", Menu: "provider", Title: "Check balance", Titles: map[string]string{"zh": "查余额"}},
		{ID: "logs", Menu: "agent"},
	}
	if len(w.Actions) != len(want) {
		t.Fatalf("actions %+v, want %+v", w.Actions, want)
	}
	for i, a := range w.Actions {
		if a.ID != want[i].ID || a.Menu != want[i].Menu || a.Title != want[i].Title || a.Titles["zh"] != want[i].Titles["zh"] {
			t.Errorf("action %d: %+v, want %+v", i, a, want[i])
		}
	}
	if w, _ := Window(mk("nopage", `{"panels":[{"slot":"agents","file":"index.html"}],"actions":[{"id":"a","menu":"agent"}]}`)); w == nil || len(w.Actions) != 0 {
		t.Errorf("a plugin with no page: %+v, want panels and no actions", w)
	}
	many := `{"page":{"file":"index.html"},"actions":[`
	for i := range 12 {
		if i > 0 {
			many += ","
		}
		many += `{"id":"a` + string(rune('a'+i)) + `","menu":"agent"}`
	}
	if w, _ := Window(mk("many", many+`]}`)); w == nil || len(w.Actions) != maxActions {
		t.Errorf("12 actions: %+v, want %d", w, maxActions)
	}
}
