package gateway

import (
	"net/http/httptest"
	"strings"
	"testing"
)

func TestCaptureRequestBodyLimit(t *testing.T) {
	body := []byte(strings.Repeat("x", callBodyLimit+17))
	got, truncated := captureRequestBody(body)
	if len(got) != callBodyLimit || !truncated {
		t.Fatalf("captured %d bytes, truncated=%v", len(got), truncated)
	}
}

func TestCaptureResponseWriterPreservesResponse(t *testing.T) {
	r := httptest.NewRecorder()
	w := &captureResponseWriter{ResponseWriter: r}
	_, _ = w.Write([]byte(`{"ok":true}`))
	if got := w.body.text(); got != `{"ok":true}` {
		t.Fatalf("capture = %q", got)
	}
	if got := r.Body.String(); got != `{"ok":true}` {
		t.Fatalf("response = %q", got)
	}
}
