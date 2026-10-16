package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"testing"
)

// A download that drops part way is tried again, and reports its progress.
func TestDownloadRetriesAndReports(t *testing.T) {
	body := make([]byte, 256<<10)
	for i := range body {
		body[i] = byte(i)
	}
	sum := sha256.Sum256(body)
	tries := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		tries++
		w.Header().Set("Content-Length", strconv.Itoa(len(body)))
		if tries == 1 {
			w.Write(body[:1000]) // then the connection goes
			return
		}
		w.Write(body)
	}))
	defer srv.Close()

	var done, total int64
	ctx := WithProgress(context.Background(), func(d, t int64) { done, total = d, t })
	path := filepath.Join(t.TempDir(), "magpie.new")
	if err := download(ctx, Asset{URL: srv.URL, SHA256: hex.EncodeToString(sum[:])}, path); err != nil {
		t.Fatal(err)
	}
	if tries != 2 {
		t.Fatalf("tries = %d, want 2", tries)
	}
	if done != int64(len(body)) || total != int64(len(body)) {
		t.Fatalf("progress = %d/%d, want %d/%d", done, total, len(body), len(body))
	}
	if b, _ := os.ReadFile(path); len(b) != len(body) {
		t.Fatalf("file has %d bytes", len(b))
	}
}
