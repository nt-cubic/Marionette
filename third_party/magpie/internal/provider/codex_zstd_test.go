package provider

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/klauspost/compress/zstd"
)

// A ChatGPT turn goes as Codex's client sends it (openai/codex
// codex-rs/http-client/src/request.rs: zstd level 3, Content-Encoding:
// zstd), and only a turn: CodexBase's /responses, POSTed, not already
// encoded. Anything else is left as it was.
func TestCodexCompressed(t *testing.T) {
	t.Cleanup(CodexZstdForTests(true))
	was := CodexBase
	CodexBase = "https://chatgpt.example/backend-api/codex"
	t.Cleanup(func() { CodexBase = was })
	body := bytes.Repeat([]byte(`{"model":"gpt-6-luna","input":"the same words again "}`), 20000)

	req, _ := http.NewRequest("POST", CodexBase+"/responses", bytes.NewReader(body))
	n := CodexCompressed(req, body)
	wire, _ := io.ReadAll(req.Body)
	if req.Header.Get("Content-Encoding") != "zstd" || n != len(wire) || req.ContentLength != int64(n) || n >= len(body)/10 {
		t.Fatalf("sent %d of %d bytes as %q, Content-Length %d", n, len(body), req.Header.Get("Content-Encoding"), req.ContentLength)
	}
	d, _ := zstd.NewReader(nil)
	defer d.Close()
	if out, err := d.DecodeAll(wire, nil); err != nil || !bytes.Equal(out, body) {
		t.Fatalf("decoded %d bytes, err %v", len(out), err)
	}
	again, _ := req.GetBody()
	if b, _ := io.ReadAll(again); !bytes.Equal(b, wire) {
		t.Error("GetBody gives another body")
	}

	for _, u := range []string{CodexBase + "/responses/compact", CodexBase + "/models", "https://api.openai.com/v1/responses", "http://127.0.0.1:1/backend-api/codex/responses"} {
		req, _ := http.NewRequest("POST", u, bytes.NewReader(body))
		if n := CodexCompressed(req, body); n != len(body) || req.Header.Get("Content-Encoding") != "" {
			t.Errorf("%s compressed", u)
		}
	}
	req, _ = http.NewRequest("POST", CodexBase+"/responses", bytes.NewReader(body))
	req.Header.Set("Content-Encoding", "gzip")
	if n := CodexCompressed(req, body); n != len(body) || req.Header.Get("Content-Encoding") != "gzip" {
		t.Error("an encoded body encoded again")
	}
}

// The warm-up "hi" is a turn as Codex's are, and goes compressed too.
func TestCodexWarmGoesZstd(t *testing.T) {
	t.Cleanup(CodexZstdForTests(true))
	var enc string
	var got map[string]any
	fake := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		enc = r.Header.Get("Content-Encoding")
		d, _ := zstd.NewReader(r.Body)
		defer d.Close()
		json.NewDecoder(d).Decode(&got)
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n")
	}))
	defer fake.Close()
	was := CodexBase
	CodexBase = fake.URL + "/backend-api/codex"
	t.Cleanup(func() { CodexBase = was })

	if err := warmCodex(context.Background(), func(context.Context, *http.Request, []byte) error { return nil }, "gpt-6-luna", ""); err != nil {
		t.Fatal(err)
	}
	if enc != "zstd" || got["model"] != "gpt-6-luna" || got["input"] == nil {
		t.Fatalf("warmed %q: %v", enc, got)
	}
}
