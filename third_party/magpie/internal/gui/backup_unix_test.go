//go:build !windows

package gui

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/davsync"
	"github.com/yetone/magpie/internal/settings"
)

// Sync now, while another magpie syncs longer than the page waits: the
// status the page is sent says so, as it says a sync that failed.
func TestDavsyncBusy(t *testing.T) {
	sandboxHome(t)
	if err := davsync.Configure(davsync.Config{URL: "http://127.0.0.1:1/dav/", Passphrase: "correct horse"}); err != nil {
		t.Fatal(err)
	}
	// the other magpie: the lock davsync takes on sync.lock, held
	f, err := os.OpenFile(filepath.Join(settings.Dir(), "sync.lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX); err != nil {
		t.Fatal(err)
	}

	mux := http.NewServeMux()
	backupRoutes(mux, folderOnly{})
	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	defer cancel()
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/davsync/now", strings.NewReader("{}")).WithContext(ctx))
	var v davsync.View
	json.Unmarshal(w.Body.Bytes(), &v)
	if w.Code != 200 || !v.On || !strings.Contains(v.Error, "another magpie is syncing") {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
}
