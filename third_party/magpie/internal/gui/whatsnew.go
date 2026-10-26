package gui

import (
	"context"
	"encoding/json"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/update"
)

// whatsNew shows what changed once magpie runs a newer release than it last
// did (a Discord user: to see whether their issue was fixed). The version it
// last ran is kept in magpie's own folder; as it starts on a newer one, the
// notes of every release since are asked of the site (where the update feed
// is) the first time a window or magpie web's page asks, and shown once.
// Offline, nothing is shown. Settings can open them again. The notes are
// in the page's language, Chinese or English, where a release has them in
// it (freecss on Discord: they should follow Settings' language).
//
// The dialog comes up by itself only on the first start of a version the
// user didn't ask for from inside magpie (#525: releases come many a day,
// and a dialog after every "Restart to update" is in the way): a restart
// into an update leaves a note of the version it installs, and that version
// starting finds it and keeps quiet; the notes are still a click away in
// Settings. The dialog's "Don't show again today" keeps it quiet for the
// rest of the local day.
type whatsNew struct {
	mu      sync.Mutex
	after   string                   // the version last run, "" for the current one's notes alone
	pending bool                     // an upgrade whose notes are still to be shown
	quiet   bool                     // the upgrade was the user's own restart to update: not shown by itself
	shown   bool                     // a page has shown them
	notes   map[string][]update.Note // by language and channel (notesKey)
	back    map[string]backNotes     // Settings' "this version and n before it", by language
}

// backNotes is the longest "this version and n before it" asked for in a
// language: a shorter one is cut from it, a longer one asked for again.
type backNotes struct {
	n    int
	list []update.Note
}

var news = &whatsNew{}

// started is when this process began, to tell magpie's folder from before
// it apart from one this run just made.
var started = time.Now()

const lastRunFile = "last-version"

// updatedInAppFile holds the version a restart to update installed, for
// that version's first start to keep its notes to Settings (#525).
const updatedInAppFile = "updated-in-app"

// quietDayFile holds the local day ("2006-01-02") the dialog's "Don't show
// again today" was ticked on.
const quietDayFile = "whatsnew-quiet-day"

// today is the local day, a variable for tests.
var today = func() string { return time.Now().Format("2006-01-02") }

// updatedInApp notes that a restart to update is installing version.
func updatedInApp(version string) {
	if version == "" {
		return
	}
	dir := settings.Dir()
	if err := os.MkdirAll(dir, 0o755); err == nil {
		err = os.WriteFile(filepath.Join(dir, updatedInAppFile), []byte(version+"\n"), 0o644)
		if err != nil {
			log.Println("updated in app:", err)
		}
	}
}

// quietToday is whether "Don't show again today" was ticked today.
func quietToday() bool {
	b, err := os.ReadFile(filepath.Join(settings.Dir(), quietDayFile))
	return err == nil && strings.TrimSpace(string(b)) == today()
}

// setQuietToday ticks (or unticks) "Don't show again today".
func setQuietToday(on bool) error {
	f := filepath.Join(settings.Dir(), quietDayFile)
	if !on {
		if err := os.Remove(f); err != nil && !os.IsNotExist(err) {
			return err
		}
		return nil
	}
	if err := os.MkdirAll(settings.Dir(), 0o755); err != nil {
		return err
	}
	return os.WriteFile(f, []byte(today()+"\n"), 0o644)
}

// start reads the version last run and puts this one in its place.
func (n *whatsNew) start() {
	dir := settings.Dir()
	b, _ := os.ReadFile(filepath.Join(dir, lastRunFile))
	last := strings.TrimSpace(string(b))
	after, show := update.ShowNotes(last, Version, last == "" && usedBefore(dir))
	// a restart to update into this version: its notes wait in Settings
	mark := filepath.Join(dir, updatedInAppFile)
	m, err := os.ReadFile(mark)
	inApp := err == nil && strings.TrimSpace(string(m)) == Version
	if err == nil && (inApp || update.Released(Version)) {
		os.Remove(mark) // read once; another version's is stale
	}
	n.mu.Lock()
	n.after, n.pending, n.quiet = after, show, inApp
	n.mu.Unlock()
	if !update.Released(Version) || last == Version {
		return // a build from source leaves the release it came after
	}
	if err := os.MkdirAll(dir, 0o755); err == nil {
		err = os.WriteFile(filepath.Join(dir, lastRunFile), []byte(Version+"\n"), 0o644)
		if err != nil {
			log.Println("last version:", err)
		}
	}
}

// usedBefore is whether magpie's folder held its files before this run: a
// magpie from before the version was kept, not a fresh install.
func usedBefore(dir string) bool {
	for _, f := range []string{"settings.json", "providers.json"} {
		if fi, err := os.Stat(filepath.Join(dir, f)); err == nil && fi.ModTime().Before(started) {
			return true
		}
	}
	return false
}

type whatsNewJSON struct {
	Show     bool          `json:"show"`
	Current  string        `json:"current"`
	Releases []update.Note `json:"releases"`
	// Error: the notes couldn't be had (the site down, or GitHub failing
	// it), as opposed to releases that have none (#661); URL is the
	// current version's release page, where they can be read instead
	Error string `json:"error,omitempty"`
	URL   string `json:"url,omitempty"`
}

// get is what a page shows: after an upgrade, the notes since, once (a
// page showing them says so with seen); asked again (all, from Settings),
// those or the current version's alone.
func (n *whatsNew) get(ctx context.Context, all bool, lang string) whatsNewJSON {
	n.mu.Lock()
	after, pending, shown := n.after, n.pending, n.shown
	if !pending {
		after = ""
	}
	key := notesKey(lang)
	notes := n.notes[key]
	// shown by itself: not after a restart to update, nor on a day it was
	// asked not to be
	popup := pending && !shown && !n.quiet && !quietToday()
	n.mu.Unlock()
	j := whatsNewJSON{Current: Version, Releases: []update.Note{}}
	if (!popup && !all) || !update.Released(Version) {
		return j
	}
	j.URL = update.ReleasePage(Version)
	if notes == nil {
		var err error
		notes, err = fetchNotes(ctx, after, Version, lang)
		if err != nil {
			j.Error = err.Error()
		}
		if len(notes) > 0 { // kept: they don't change while magpie runs
			n.mu.Lock()
			if n.notes == nil {
				n.notes = map[string][]update.Note{}
			}
			n.notes[key] = notes
			n.mu.Unlock()
		}
	}
	if len(notes) > 0 {
		j.Releases = notes
	}
	j.Show = popup && len(j.Releases) > 0
	return j
}

// notesKey is what the notes since the last update are kept by: their
// language, and the channel, whose cap on how many are shown (MostNotes)
// differs.
func notesKey(lang string) string {
	if update.Stable() {
		return lang + "|" + settings.UpdateStable
	}
	return lang
}

// before is Settings' other ranges (Linka on Discord): this version's notes
// and those of the count releases before it, newest first. They are asked
// of the site once per language for the longest count asked so far, and
// kept while magpie runs, as the releases up to this one don't change. A
// failed ask is said (Error), with the notes of a shorter range asked
// before, if any, so a page keeps what it had.
func (n *whatsNew) before(ctx context.Context, count int, lang string) whatsNewJSON {
	j := whatsNewJSON{Current: Version, Releases: []update.Note{}}
	if !update.Released(Version) {
		return j
	}
	j.URL = update.ReleasePage(Version)
	n.mu.Lock()
	got, ok := n.back[lang]
	n.mu.Unlock()
	if !ok || got.n < count {
		ctx, cancel := context.WithTimeout(ctx, 15*time.Second)
		list, err := update.NotesBack(ctx, Version, count, lang)
		cancel()
		if err != nil {
			log.Println("release notes:", err)
			j.Error = err.Error()
		} else {
			got, ok = backNotes{count, list}, true
			n.mu.Lock()
			if n.back == nil {
				n.back = map[string]backNotes{}
			}
			if n.back[lang].n < count {
				n.back[lang] = got
			}
			n.mu.Unlock()
		}
	}
	if ok {
		j.Releases = update.Back(got.list, Version, count)
	}
	return j
}

// backCount is the range a page asks for (?last=10, 20, 50 or 100), 0 for
// the notes since the last update.
func backCount(r *http.Request) int {
	for _, c := range update.BackCounts {
		if r.URL.Query().Get("last") == strconv.Itoa(c) {
			return c
		}
	}
	return 0
}

// withWaiting puts first, in Settings' What's new while an update waits,
// the notes of every release after this one up to it, marked not installed
// yet. Only the update's own notes were (小八 on Discord: on 0.1.1398 with
// 0.1.1409 waiting, 0.1.1399 to 0.1.1408 were missing), so they come from
// the site as the restart's dialog has them (update/notes), with the
// update's own when the site can't say.
func withWaiting(ctx context.Context, list []update.Note, u updateJSON, lang string) []update.Note {
	switch u.State {
	case "ready", "available", "downloading", "found":
	default:
		return list
	}
	if !update.Released(Version) || u.Latest == "" || !update.Newer(u.Latest, Version) {
		return list
	}
	ahead, _ := fetchNotes(ctx, Version, u.Latest, lang)
	if len(ahead) == 0 && strings.TrimSpace(u.Notes) != "" {
		ahead = []update.Note{{Version: u.Latest, Notes: u.Notes, URL: u.URL}}
	}
	out := make([]update.Note, 0, len(ahead)+len(list))
	for _, r := range ahead {
		if !slices.ContainsFunc(list, func(o update.Note) bool { return o.Version == r.Version }) {
			r.Pending = true
			out = append(out, r)
		}
	}
	out = append(out, list...)
	if most := update.MostNotes(); len(out) > most {
		out = out[:most]
	}
	return out
}

func (n *whatsNew) seen() {
	n.mu.Lock()
	n.shown = true
	n.mu.Unlock()
}

// fetchNotes asks the site for the notes since after; a site without the
// list yet, or one that can't be reached, leaves the update feed, whose
// newest release has its notes, for when that is the current one. The
// error says the notes couldn't be had, which the page tells apart from
// releases without notes (#661): the list failed and the feed had none.
func fetchNotes(ctx context.Context, after, upto, lang string) ([]update.Note, error) {
	ctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	notes, failed := update.NotesBetween(ctx, after, upto, lang)
	if failed == nil && len(notes) > 0 {
		return notes, nil
	}
	if failed != nil {
		log.Println("release notes:", failed)
	}
	rel, err := update.LatestIn(ctx, lang)
	if err != nil {
		return nil, failed
	}
	notes = update.Between([]update.Note{{Version: rel.Version, Notes: rel.Notes, URL: rel.URL}}, after, upto)
	if len(notes) == 0 {
		return nil, failed
	}
	return notes, nil
}

// pageLang is the language a page asks in, its own (?lang=, zh or en), or
// without one the app's, as the tray menu has it, in the languages release
// notes come in.
func pageLang(r *http.Request) string {
	if l := askedLang(r); l != "" {
		return l
	}
	return notesLang(trayLang(settings.Load().Lang, systemLang))
}

// askedLang is the language a page names (?lang=): zh for any Chinese, en
// for anything else, "" for none.
func askedLang(r *http.Request) string {
	switch l := strings.ToLower(r.URL.Query().Get("lang")); {
	case strings.HasPrefix(l, "zh"):
		return "zh"
	case l != "":
		return "en"
	}
	return ""
}

func whatsNewRoutes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/whatsnew", func(rw http.ResponseWriter, r *http.Request) {
		all, lang := r.URL.Query().Get("all") != "", pageLang(r)
		if c := backCount(r); all && c > 0 {
			writeJSON(rw, news.before(r.Context(), c, lang))
			return
		}
		j := news.get(r.Context(), all, lang)
		if all {
			j.Releases = withWaiting(r.Context(), j.Releases, updates.jsonIn(askedLang(r)), lang)
		}
		writeJSON(rw, j)
	})
	mux.HandleFunc("POST /api/whatsnew/seen", func(rw http.ResponseWriter, r *http.Request) {
		news.seen()
		rw.WriteHeader(http.StatusNoContent)
	})
	// today: the dialog's "Don't show again today", ticked or unticked
	mux.HandleFunc("POST /api/whatsnew/today", func(rw http.ResponseWriter, r *http.Request) {
		var in struct {
			On bool `json:"on"`
		}
		if err := json.NewDecoder(r.Body).Decode(&in); err != nil {
			http.Error(rw, err.Error(), http.StatusBadRequest)
			return
		}
		if err := setQuietToday(in.On); err != nil {
			http.Error(rw, err.Error(), http.StatusInternalServerError)
			return
		}
		rw.WriteHeader(http.StatusNoContent)
	})
}
