package pii

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// The model is downloaded through the proxy set in magpie, as magpie's
// other requests are (hh0592821 on Discord: a client made before
// netproxy.Install went direct, and in China huggingface.co answered with
// a *.facebook.com certificate while their proxy got the file).
func TestFetchGoesThroughMagpiesProxy(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	for _, k := range []string{"HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"} {
		t.Setenv(k, "")
	}
	body := bytes.Repeat([]byte("model bytes "), 1000)
	var mu sync.Mutex
	var seen []string
	// the proxy: a plain http:// request comes to it whole, by its URL
	px := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r.URL.String())
		mu.Unlock()
		w.Write(body)
	}))
	defer px.Close()
	if err := settings.Save(settings.Settings{Proxy: px.URL}); err != nil {
		t.Fatal(err)
	}
	// TEST-NET-1: nothing answers there but the proxy, and no name is
	// looked up for it
	raw := "http://192.0.2.1/QuantumNous/astrlink-guard/resolve/r/int8/tokenizer.json"
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	path := filepath.Join(t.TempDir(), "tokenizer.json")
	var n atomic.Int64
	if err := fetch(ctx, raw, path, int64(len(body)), sum(body), &n); err != nil {
		t.Fatalf("fetch: %v", err)
	}
	mu.Lock()
	defer mu.Unlock()
	if !slices.Equal(seen, []string{raw}) {
		t.Fatalf("through the proxy: %v, want %s", seen, raw)
	}
}

// hosts answers each request by its host, as the world would: one host's
// answer an error, another's the file.
type hosts map[string]func(*http.Request) (*http.Response, error)

func (h hosts) RoundTrip(r *http.Request) (*http.Response, error) {
	if f, ok := h[r.URL.Host]; ok {
		return f(r)
	}
	return nil, errors.New("no such host " + r.URL.Host)
}

func serving(b []byte) func(*http.Request) (*http.Response, error) {
	return func(r *http.Request) (*http.Response, error) {
		return &http.Response{StatusCode: 200, Status: "200 OK", Header: http.Header{}, Body: io.NopCloser(bytes.NewReader(b)), ContentLength: int64(len(b)), Request: r}, nil
	}
}

// Where huggingface.co can't be reached (hh0592821's poisoned answer, a
// handshake that times out, EOF), the masking model comes from hf-mirror,
// with the 「国内镜像」 switch off; and what hf-mirror serves is checked
// against the pin all the same.
func TestModelFromTheMirrorWhenHuggingFaceFails(t *testing.T) {
	body := bytes.Repeat([]byte("guard bytes "), 1000)
	was := client
	t.Cleanup(func() { client = was })
	raw := guardBase + guardRev + "/int8/tokenizer.json"
	for _, fail := range []error{
		errors.New("tls: failed to verify certificate: x509: certificate is valid for *.facebook.com, facebook.com, not huggingface.co"),
		errors.New("net/http: TLS handshake timeout"),
		io.EOF,
	} {
		var asked []string
		var mu sync.Mutex
		note := func(f func(*http.Request) (*http.Response, error)) func(*http.Request) (*http.Response, error) {
			return func(r *http.Request) (*http.Response, error) {
				mu.Lock()
				asked = append(asked, r.URL.String())
				mu.Unlock()
				return f(r)
			}
		}
		client = &http.Client{Transport: hosts{
			"huggingface.co": note(func(*http.Request) (*http.Response, error) { return nil, fail }),
			"hf-mirror.com":  note(serving(body)),
		}}
		path := filepath.Join(t.TempDir(), "tokenizer.json")
		var n atomic.Int64
		if err := fetch(context.Background(), raw, path, int64(len(body)), sum(body), &n); err != nil {
			t.Fatalf("%v: %v", fail, err)
		}
		want := []string{raw, strings.Replace(raw, "https://huggingface.co", "https://hf-mirror.com", 1)}
		if !slices.Equal(asked, want) {
			t.Fatalf("%v: asked %v, want %v", fail, asked, want)
		}
	}

	// a mirror's file that isn't the pinned one is not kept
	client = &http.Client{Transport: hosts{
		"huggingface.co": func(*http.Request) (*http.Response, error) { return nil, io.EOF },
		"hf-mirror.com":  serving(bytes.Repeat([]byte("other bytes "), 1000)),
	}}
	path := filepath.Join(t.TempDir(), "tokenizer.json")
	var n atomic.Int64
	err := fetch(context.Background(), raw, path, int64(len(body)), sum(body), &n)
	if err == nil || !strings.Contains(err.Error(), "checksum") {
		t.Fatalf("a mirror's wrong file: %v", err)
	}
}
