package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/agent"
)

type importWindows struct {
	Windows
	opened string
}

func (w *importWindows) OpenURL(u string) error { w.opened = u; return nil }

func TestLodyImportHandoff(t *testing.T) {
	home := sandboxHome(t)
	// no Lody desktop entry of the machine's is run (agent.OpenLody)
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, ".local", "share"))
	t.Setenv("XDG_DATA_DIRS", filepath.Join(home, "share"))
	if err := os.MkdirAll(filepath.Join(home, ".lody"), 0700); err != nil {
		t.Fatal(err)
	}
	catalog := filepath.Join(home, ".lody", "workspace-catalog.json")
	if err := os.WriteFile(catalog, []byte(`{"workspaces":[]}`), 0600); err != nil {
		t.Fatal(err)
	}
	// Cindy, beside it, still says whether it has magpie: false is said,
	// not left out as unknown
	cfg, err := os.UserConfigDir()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(cfg, "Cindy"), 0700); err != nil {
		t.Fatal(err)
	}
	var row, cindy *agentJSON
	s := state()
	for i := range s.Agents {
		switch s.Agents[i].ID {
		case "lody":
			row = &s.Agents[i]
		case "cindy":
			cindy = &s.Agents[i]
		}
	}
	if row == nil || !strings.HasPrefix(row.Import, agent.LodyScheme) {
		t.Fatalf("missing Lody import row: %+v", row)
	}
	if row.Added != nil {
		t.Fatal("unknown import status must not be reported as not added")
	}
	if b, _ := json.Marshal(row); strings.Contains(string(b), `"added"`) {
		t.Fatalf("Lody's row says added: %s", b)
	}
	if cindy == nil || cindy.Added == nil || *cindy.Added {
		t.Fatalf("Cindy without magpie: %+v, want added false", cindy)
	}
	if b, _ := json.Marshal(cindy); !strings.Contains(string(b), `"added":false`) {
		t.Fatalf("Cindy's row leaves added out: %s", b)
	}
	w := &importWindows{}
	mux := http.NewServeMux()
	providerRoutes(mux, w)
	for _, link := range []string{row.Import, "lody://session/example", "lody://provider/delete?all=1", "file:///tmp/example"} {
		w.opened = ""
		body, _ := json.Marshal(map[string]string{"url": link})
		rec := httptest.NewRecorder()
		mux.ServeHTTP(rec, httptest.NewRequest("POST", "/api/open", strings.NewReader(string(body))))
		if link == row.Import {
			if w.opened != link {
				t.Fatal("Lody import was not handed to the desktop")
			}
		} else if w.opened != "" {
			t.Fatalf("opened unrelated URL %s", w.opened)
		}
	}
	entries, err := os.ReadDir(filepath.Join(home, ".lody"))
	if err != nil || len(entries) != 1 {
		t.Fatalf("handoff modified Lody data: %v %v", entries, err)
	}
	if b, err := os.ReadFile(catalog); err != nil || string(b) != `{"workspaces":[]}` {
		t.Fatalf("handoff modified Lody's catalog: %q %v", b, err)
	}
}

// On Linux the window's Add magpie runs Lody with its link, as Lody's
// desktop entry says, rather than handing lody:// to xdg-open, which turned
// it away while Lody ran (Atlas, 10-25: "failed to open uri"): Lody's
// AppImage makes itself the default only for ai.lody.stable://. The entry
// is the one Lody 0.108.1's AppImage writes, with a fake AppImage.
func TestLodyImportRunsLodyOnLinux(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("desktop entries are Linux's; macOS and Windows open lody:// (agent.TestOpenLodyRunsItsDesktopEntry)")
	}
	home := sandboxHome(t)
	data := filepath.Join(home, ".local", "share")
	t.Setenv("XDG_DATA_HOME", data)
	t.Setenv("XDG_DATA_DIRS", filepath.Join(home, "share"))
	got := filepath.Join(home, "got")
	appImage := filepath.Join(home, "Applications", "Lody.AppImage")
	if err := os.MkdirAll(filepath.Dir(appImage), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(appImage, []byte("#!/bin/sh\nprintf '%s\\n' \"$@\" > "+got+"\n"), 0755); err != nil {
		t.Fatal(err)
	}
	apps := filepath.Join(data, "applications")
	if err := os.MkdirAll(apps, 0700); err != nil {
		t.Fatal(err)
	}
	entry := "[Desktop Entry]\nName=Lody\nExec=\"" + appImage + "\" \"--no-sandbox\" %U\nTerminal=false\nType=Application\nMimeType=x-scheme-handler/ai.lody.stable;x-scheme-handler/lody;\nCategories=Utility;\n"
	if err := os.WriteFile(filepath.Join(apps, "ai.lody.desktop.desktop"), []byte(entry), 0644); err != nil {
		t.Fatal(err)
	}
	link := agent.LodyLink("http://127.0.0.1:3425")
	w := &importWindows{}
	mux := http.NewServeMux()
	providerRoutes(mux, w)
	body, _ := json.Marshal(map[string]string{"url": link})
	rec := httptest.NewRecorder()
	mux.ServeHTTP(rec, httptest.NewRequest("POST", "/api/open", strings.NewReader(string(body))))
	if rec.Code != http.StatusNoContent {
		t.Fatalf("open answered %d %s", rec.Code, rec.Body)
	}
	if w.opened != "" {
		t.Fatalf("the link went to xdg-open: %s", w.opened)
	}
	want := "--no-sandbox\n" + link + "\n"
	var b []byte
	for deadline := time.Now().Add(30 * time.Second); string(b) != want && time.Now().Before(deadline); {
		time.Sleep(20 * time.Millisecond)
		b, _ = os.ReadFile(got)
	}
	if string(b) != want {
		t.Fatalf("Lody ran with %q, want %q", b, want)
	}
}

// Cindy's database that can't be read leaves its row's added out, as
// unknown: the window never says "Not added" for what it couldn't read.
func TestImportStatusUnreadableIsUnknown(t *testing.T) {
	sandboxHome(t)
	cfg, err := os.UserConfigDir()
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(cfg, "Cindy")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "cindy-local-v1.db"), []byte("half-written, no database"), 0600); err != nil {
		t.Fatal(err)
	}
	for _, a := range state().Agents {
		if a.ID == "cindy" {
			if b, _ := json.Marshal(a); strings.Contains(string(b), `"added"`) {
				t.Fatalf("unreadable Cindy database said: %s", b)
			}
			return
		}
	}
	t.Fatal("no Cindy row")
}

// AionUi's row is its add-provider link, opened as it is and no other
// aionui:// link; with AionUi's folder and no database yet it says not
// added, and with a database it can't read it says nothing.
func TestAionUiImportRow(t *testing.T) {
	sandboxHome(t)
	cfg, err := os.UserConfigDir()
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(cfg, "AionUi", "aionui")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	find := func() *agentJSON {
		s := state()
		for i := range s.Agents {
			if s.Agents[i].ID == "aionui" {
				return &s.Agents[i]
			}
		}
		return nil
	}
	row := find()
	if row == nil || !strings.HasPrefix(row.Import, agent.AionUiScheme) || !strings.Contains(row.Import, "platform=custom") {
		t.Fatalf("AionUi's row: %+v", row)
	}
	if row.Added == nil || *row.Added {
		t.Fatalf("AionUi with no database: %+v, want added false", row.Added)
	}
	if err := os.WriteFile(filepath.Join(dir, "aionui-backend.db"), []byte("not a database"), 0o600); err != nil {
		t.Fatal(err)
	}
	if b, _ := json.Marshal(find()); strings.Contains(string(b), `"added"`) {
		t.Fatalf("an unreadable database says added: %s", b)
	}
	w := &importWindows{}
	mux := http.NewServeMux()
	providerRoutes(mux, w)
	for _, link := range []string{row.Import, "aionui://other?x=1", "aionui://add-provider-x?a=1", "aionui:add-provider?a=1"} {
		w.opened = ""
		body, _ := json.Marshal(map[string]string{"url": link})
		mux.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("POST", "/api/open", strings.NewReader(string(body))))
		if link == row.Import {
			if w.opened != link {
				t.Fatal("AionUi's link was not handed to the desktop")
			}
		} else if w.opened != "" {
			t.Fatalf("opened unrelated URL %s", w.opened)
		}
	}
}
