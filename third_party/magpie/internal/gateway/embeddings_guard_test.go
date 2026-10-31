package gateway

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/settings"
)

// #4553 (SunnyRx, Windows): with AstrLink Guard masking requests, an
// embeddings input with a stray byte (the reporter's body, a lone 0xff)
// panicked in the model's tokenizer, on the model's own goroutine, and
// magpie exited with code 2. The real model reads it now, the vendor gets
// the request, and the next request is served. It needs the model's
// files, in MAGPIE_GUARD_DIR, as internal/pii/guard's tests do.
func TestEmbeddingsWithAStrayByteUnderAstrLinkGuard(t *testing.T) {
	src := os.Getenv("MAGPIE_GUARD_DIR")
	if src == "" {
		t.Skip("MAGPIE_GUARD_DIR is not set")
	}
	s, l := shelved(t)
	// where pii looks for the pinned revision's files
	dst := filepath.Join(appdir.Cache(), "astrlink-guard", "7216b6a3b897")
	if err := os.MkdirAll(dst, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, f := range []string{"model_int8.onnx", "model_int8.onnx_data", "tokenizer.json"} {
		copyFile(t, filepath.Join(src, f), filepath.Join(dst, f))
	}
	g := pii.Of(pii.AstrLinkGuard)
	if !g.Installed() {
		t.Fatalf("the model isn't installed from %s: the pinned revision moved, and this test's folder with it", dst)
	}
	t.Cleanup(g.Stop)
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactModel: true, RedactModelName: pii.AstrLinkGuard}); err != nil {
		t.Fatal(err)
	}
	body := `{"model":"lib/embed-1","input":["hello ` + string([]byte{0xff}) + `","world"]}`
	if code, raw := postRetrieval(t, s, "/v1/embeddings", body); code != 200 {
		t.Fatalf("%d %s", code, raw)
	}
	if code, raw := postRetrieval(t, s, "/v1/embeddings", `{"model":"lib/embed-1","input":["call 13800138000","world"]}`); code != 200 {
		t.Fatalf("the next request: %d %s", code, raw)
	}
	sent := l.got("/v1/embeddings")
	if len(sent) != 2 || !strings.Contains(sent[0], "hello") || strings.Contains(sent[1], "13800138000") {
		t.Fatalf("vendor was sent %q", sent)
	}
	if st := g.Now(); !st.Running {
		t.Fatalf("the model didn't read the requests: %+v", st)
	}
}

func copyFile(t *testing.T, from, to string) {
	t.Helper()
	in, err := os.Open(from)
	if err != nil {
		t.Fatal(err)
	}
	defer in.Close()
	out, err := os.Create(to)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := io.Copy(out, in); err != nil {
		t.Fatal(err)
	}
	if err := out.Close(); err != nil {
		t.Fatal(err)
	}
}
