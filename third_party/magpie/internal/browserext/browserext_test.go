package browserext

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The extension is written pointed at the gateway it is set up from, and
// written again only where it changed.
func TestWritePointsAtTheGateway(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "ext")
	if err := Write(dir, "http://127.0.0.1:3425"); err != nil {
		t.Fatal(err)
	}
	cfg, _ := os.ReadFile(filepath.Join(dir, "config.js"))
	if !strings.Contains(string(cfg), `export const GATEWAY = "ws://127.0.0.1:3425";`) {
		t.Fatalf("config.js: %s", cfg)
	}
	var man struct {
		Version, Key string
		Background   struct {
			ServiceWorker string `json:"service_worker"`
		}
	}
	b, _ := os.ReadFile(filepath.Join(dir, "manifest.json"))
	if err := json.Unmarshal(b, &man); err != nil || man.Version != Version() || man.Key == "" || man.Background.ServiceWorker != "background.js" {
		t.Fatalf("manifest: %v %+v", err, man)
	}
	old := time.Now().Add(-time.Hour)
	bg := filepath.Join(dir, "background.js")
	os.Chtimes(bg, old, old)
	if err := Write(dir, "http://127.0.0.1:4000/"); err != nil {
		t.Fatal(err)
	}
	cfg, _ = os.ReadFile(filepath.Join(dir, "config.js"))
	if !strings.Contains(string(cfg), `"ws://127.0.0.1:4000"`) {
		t.Fatalf("another port: %s", cfg)
	}
	if fi, _ := os.Stat(bg); !fi.ModTime().Equal(old) {
		t.Fatal("background.js was written again unchanged")
	}
	if Write(dir, "ftp://x") == nil {
		t.Fatal("took an address that isn't http")
	}
}
