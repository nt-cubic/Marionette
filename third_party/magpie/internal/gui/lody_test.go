package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/agent"
)

type importWindows struct {
	Windows
	opened string
}

func (w *importWindows) OpenURL(u string) error { w.opened = u; return nil }

func TestLodyImportHandoff(t *testing.T) {
	home := sandboxHome(t)
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
