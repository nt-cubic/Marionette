package pii

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func sum(b []byte) string {
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

// files serves each of files at its path, with Range as net/http does
// it, and keeps the Range header each request carried.
type files struct {
	mu     sync.Mutex
	ranges []string
	m      map[string][]byte
}

func (f *files) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	f.ranges = append(f.ranges, r.URL.Path+" "+r.Header.Get("Range"))
	b, ok := f.m[r.URL.Path]
	f.mu.Unlock()
	if !ok {
		http.NotFound(w, r)
		return
	}
	http.ServeContent(w, r, "", time.Time{}, bytes.NewReader(b))
}

// A download stopped halfway is taken up where it stopped.
func TestFetchTakesUpThePart(t *testing.T) {
	body := bytes.Repeat([]byte("model bytes "), 1000)
	f := &files{m: map[string][]byte{"/m": body}}
	s := httptest.NewServer(f)
	defer s.Close()
	path := filepath.Join(t.TempDir(), "m.gguf")
	os.WriteFile(path+".part", body[:5000], 0o644)
	var n atomic.Int64
	if err := fetch(context.Background(), s.URL+"/m", path, int64(len(body)), sum(body), &n); err != nil {
		t.Fatal(err)
	}
	if got, _ := os.ReadFile(path); !bytes.Equal(got, body) {
		t.Fatal("the file isn't what was served")
	}
	if f.ranges[0] != "/m bytes=5000-" {
		t.Fatalf("asked %q, want from byte 5000 on", f.ranges)
	}
	if n.Load() != int64(len(body)) {
		t.Fatalf("done = %d", n.Load())
	}
	if _, err := os.Stat(path + ".part"); err == nil {
		t.Fatal("the part was left")
	}
}

// A part that wasn't this file's fails the checksum, and is thrown away so
// that the next try starts over.
func TestFetchChecksumMismatch(t *testing.T) {
	body := bytes.Repeat([]byte("model bytes "), 1000)
	s := httptest.NewServer(&files{m: map[string][]byte{"/m": body}})
	defer s.Close()
	path := filepath.Join(t.TempDir(), "m.gguf")
	os.WriteFile(path+".part", bytes.Repeat([]byte("x"), 5000), 0o644)
	var n atomic.Int64
	err := fetch(context.Background(), s.URL+"/m", path, int64(len(body)), sum(body), &n)
	if err == nil || !strings.Contains(err.Error(), "checksum") {
		t.Fatalf("err = %v", err)
	}
	if _, err := os.Stat(path); err == nil {
		t.Fatal("a file that failed its checksum was kept")
	}
	if _, err := os.Stat(path + ".part"); err == nil {
		t.Fatal("the bad part was kept")
	}
	if err := fetch(context.Background(), s.URL+"/m", path, int64(len(body)), sum(body), &n); err != nil {
		t.Fatalf("the next try: %v", err)
	}
}

// A server that stops sending is given up on after stallAfter, and what
// it sent is kept for the next try.
func TestFetchStall(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Length", "100000")
		w.Write(make([]byte, 1000))
		w.(http.Flusher).Flush()
		select {
		case <-r.Context().Done():
		case <-release:
		}
	}))
	defer s.Close()
	was := stallAfter
	stallAfter = 200 * time.Millisecond
	t.Cleanup(func() { stallAfter = was })
	path := filepath.Join(t.TempDir(), "m.gguf")
	var n atomic.Int64
	err := fetch(context.Background(), s.URL+"/m", path, 100000, strings.Repeat("0", 64), &n)
	if err == nil || !strings.Contains(err.Error(), "no data for") {
		t.Fatalf("err = %v", err)
	}
	if fi, err := os.Stat(path + ".part"); err != nil || fi.Size() != 1000 {
		t.Fatalf("part: %v %v", fi, err)
	}
}

// Download fetches pf-serve's zip and the model, puts each file of the
// zip in serveDir by its name alone, and calls done once both are there.
func TestDownloadInstalls(t *testing.T) {
	var zb bytes.Buffer
	zw := zip.NewWriter(&zb)
	for name, body := range map[string]string{
		"pf-serve-x/" + filepath.Base(exe("pf-serve")): "serve",
		"../../escape": "out",
		"LICENSE-ggml": "mit",
		"nested/dir/":  "",
	} {
		w, _ := zw.Create(name)
		w.Write([]byte(body))
	}
	zw.Close()
	model := []byte("a small model")
	f := &files{m: map[string][]byte{
		"/rel/pf-v9/pf-serve-" + target() + ".zip": zb.Bytes(),
		"/model.gguf": model,
	}}
	s := httptest.NewServer(f)
	defer s.Close()

	d := t.TempDir()
	root := filepath.Join(d, "cache")
	wasDir, wasURL, wasSize, wasSHA := dir, modelURL, modelSize, modelSHA
	wasTag, wasBase, wasServe := serveTag, serveBase, serveSHA
	t.Cleanup(func() {
		dir, modelURL, modelSize, modelSHA = wasDir, wasURL, wasSize, wasSHA
		serveTag, serveBase, serveSHA = wasTag, wasBase, wasServe
	})
	dir = func() string { return root }
	modelURL, modelSize, modelSHA = s.URL+"/model.gguf", int64(len(model)), sum(model)
	serveTag, serveBase, serveSHA = "pf-v9", s.URL+"/rel/", map[string]string{target(): sum(zb.Bytes())}

	if Installed() {
		t.Fatal("installed before the download")
	}
	done := make(chan struct{})
	if err := Download(func() { close(done) }); err != nil {
		t.Fatal(err)
	}
	for deadline := time.Now().Add(30 * time.Second); ; time.Sleep(10 * time.Millisecond) {
		select {
		case <-done:
		default:
			if st := Now(); st.Error != "" || time.Now().After(deadline) {
				t.Fatalf("not done: %+v", st)
			}
			continue
		}
		break
	}
	if !Installed() {
		t.Fatal("not installed after the download")
	}
	if b, _ := os.ReadFile(exe("pf-serve")); string(b) != "serve" {
		t.Fatalf("pf-serve = %q", b)
	}
	if b, _ := os.ReadFile(filepath.Join(serveDir(), "LICENSE-ggml")); string(b) != "mit" {
		t.Fatalf("LICENSE-ggml = %q", b)
	}
	if _, err := os.Stat(filepath.Join(d, "escape")); err == nil {
		t.Fatal("a zip entry landed outside serveDir")
	}
	if _, err := os.Stat(filepath.Join(serveDir(), "escape")); err != nil {
		t.Fatal("a zip entry's file wasn't put in serveDir by its name")
	}
	if st := Now(); st.Downloading || st.Error != "" || st.Done != st.Total {
		t.Fatalf("Now = %+v", st)
	}
	if err := Remove(); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(root); err == nil {
		t.Fatal("Remove left the files")
	}
}

// A system pf-serve isn't built for is told so, and nothing is fetched.
func TestDownloadUnsupported(t *testing.T) {
	was := serveSHA
	serveSHA = map[string]string{}
	t.Cleanup(func() { serveSHA = was })
	if err := Download(nil); err == nil || !strings.Contains(err.Error(), "doesn't run on") {
		t.Fatalf("err = %v", err)
	}
}
