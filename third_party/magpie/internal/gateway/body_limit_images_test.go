package gateway

import (
	"bytes"
	"encoding/base64"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/klauspost/compress/zstd"
)

// NeoPengZhile's Codex session had read many images; Codex sends the
// whole conversation each turn, screenshots inline, zstd-compressed, and
// past 128 MB decoded magpie answered 413 on all five retries (#1422).
// Such a turn is read now: a zstd body of inline images decoding to
// 136 MB, on Codex's backend path, with the gateway's own limit.
func TestImageHeavyCodexTurnIsRead(t *testing.T) {
	if testing.Short() {
		t.Skip("allocates a few hundred MB")
	}
	img := base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0x89, 'P', 'N', 'G', 1, 2, 3, 4}, 1<<17)) // ~1.3 MB each
	var turn strings.Builder
	turn.WriteString(`{"model":"gpt-5.5","input":[`)
	for i := 0; turn.Len() < 136<<20; i++ {
		if i > 0 {
			turn.WriteByte(',')
		}
		turn.WriteString(`{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,`)
		turn.WriteString(img)
		turn.WriteString(`"}]}`)
	}
	turn.WriteString(`]}`)
	plain := turn.String()
	zw, err := zstd.NewWriter(nil)
	if err != nil {
		t.Fatal(err)
	}
	sent := zw.EncodeAll([]byte(plain), nil)
	r := httptest.NewRequest("POST", CodexPath+"/responses", bytes.NewReader(sent))
	r.Header.Set("Content-Encoding", "zstd")
	w := httptest.NewRecorder()
	body, status, err := readBoundedRequestBody(w, r, requestLimits{})
	if err != nil {
		t.Fatalf("a %d MB image-heavy turn (%d bytes on the wire): %d %v", len(plain)>>20, len(sent), status, err)
	}
	if len(body) != len(plain) {
		t.Fatalf("read %d bytes of %d", len(body), len(plain))
	}
}

// One past the limit is still refused, and the 413 says it is magpie's
// own limit, that the provider wasn't asked, and to compact (#1422): the
// reporter saw only "request body exceeds gateway limit" and couldn't
// tell whose it was.
func TestOverLimitSaysWhoseAndWhatToDo(t *testing.T) {
	var compressed bytes.Buffer
	zw, err := zstd.NewWriter(&compressed)
	if err != nil {
		t.Fatal(err)
	}
	zw.Write([]byte(`{"model":"gpt-5.5","input":"` + strings.Repeat("x", 4096) + `"}`))
	zw.Close()
	s := New()
	s.requestLimits.body = 1024
	r := httptest.NewRequest("POST", CodexPath+"/responses", &compressed)
	r.Header.Set("Content-Encoding", "zstd")
	w := httptest.NewRecorder()
	s.codexBackend(w, r)
	if w.Code != 413 {
		t.Fatalf("status %d, want 413", w.Code)
	}
	got := w.Body.String()
	for _, want := range []string{"magpie's gateway limit of 1 KB", "the provider was not asked", "/compact", "not the model's context"} {
		if !strings.Contains(got, want) {
			t.Errorf("413 body %s doesn't say %q", got, want)
		}
	}
}
