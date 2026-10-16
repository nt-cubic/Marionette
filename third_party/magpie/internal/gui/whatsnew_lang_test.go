package gui

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/update"
)

const (
	notesEn   = "### Features\n\n- One thing (#1)"
	notesZh   = "### 新功能\n\n- 一件事 (#1)"
	notesBoth = notesEn + "\n\n### Install\n\nDownload it.\n\n<!-- lang:zh -->\n\n" + notesZh
)

// The language a page names: zh for any Chinese, en for anything else.
func TestAskedLang(t *testing.T) {
	for q, want := range map[string]string{"": "", "?lang=zh": "zh", "?lang=zh-CN": "zh", "?lang=en": "en", "?lang=fr": "en", "?all=1&lang=zh": "zh"} {
		r := httptest.NewRequest("GET", "/api/whatsnew"+q, nil)
		if got := askedLang(r); got != want {
			t.Errorf("%q: %q, want %q", q, got, want)
		}
	}
}

// What's new follows the page's language (freecss on Discord): the notes
// are asked of the site in it, and kept for it alone; a page in another
// language gets its own.
func TestWhatsNewInPageLang(t *testing.T) {
	home := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", home)
	dir := filepath.Join(home, "magpie")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(filepath.Join(dir, lastRunFile), []byte("0.1.603\n"), 0o644)
	var mu sync.Mutex
	var asked []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		asked = append(asked, r.URL.Query().Get("lang"))
		mu.Unlock()
		json.NewEncoder(w).Encode(map[string]any{"releases": []update.Note{{Version: "0.1.604", Notes: notesBoth}}})
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_NOTES_FEED", srv.URL)
	t.Setenv("MAGPIE_UPDATE_FEED", "http://127.0.0.1:1/")
	old := Version
	defer func() { Version = old }()
	Version = "0.1.604"
	n := &whatsNew{}
	n.start()

	if j := n.get(context.Background(), false, "zh"); !j.Show || len(j.Releases) != 1 || j.Releases[0].Notes != notesZh {
		t.Fatalf("zh: %+v", j)
	}
	if j := n.get(context.Background(), true, "zh"); len(j.Releases) != 1 || j.Releases[0].Notes != notesZh {
		t.Fatalf("zh again: %+v", j)
	}
	if j := n.get(context.Background(), true, "en"); len(j.Releases) != 1 || j.Releases[0].Notes != notesEn {
		t.Fatalf("en: %+v", j)
	}
	mu.Lock()
	defer mu.Unlock()
	if strings.Join(asked, ",") != "zh,en" {
		t.Fatalf("asked in %q", asked)
	}
}

// The waiting update's notes follow the page's language too: held in
// another, they are asked for again in the page's, once.
func TestUpdateNotesInPageLang(t *testing.T) {
	var mu sync.Mutex
	var asked []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		asked = append(asked, r.URL.Query().Get("lang"))
		mu.Unlock()
		json.NewEncoder(w).Encode(update.Release{Version: "0.1.605", Notes: notesBoth})
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_UPDATE_FEED", srv.URL)
	u := &updater{state: "ready", latest: &update.Release{Version: "0.1.605", Notes: notesEn}, notesIn: "en"}
	if j := u.jsonIn("en"); j.Notes != notesEn {
		t.Fatalf("en: %q", j.Notes)
	}
	if j := u.jsonIn("zh"); j.Notes != notesEn { // until the Chinese is in
		t.Fatalf("zh at once: %q", j.Notes)
	}
	deadline := time.Now().Add(5 * time.Second)
	for u.jsonIn("zh").Notes != notesZh {
		if time.Now().After(deadline) {
			t.Fatalf("zh never came: %q", u.jsonIn("zh").Notes)
		}
		time.Sleep(10 * time.Millisecond)
	}
	if u.json().Notes != notesZh || u.lang != "zh" {
		t.Fatalf("held %q in %q", u.json().Notes, u.lang)
	}
	mu.Lock()
	defer mu.Unlock()
	if strings.Join(asked, ",") != "zh" {
		t.Fatalf("asked in %q", asked)
	}
}
