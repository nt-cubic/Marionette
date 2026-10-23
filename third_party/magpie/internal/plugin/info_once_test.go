package plugin

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// A request is told the provider as info builds it once for its account,
// not built again for every fetch: each build asked the plugin's models
// hook, and with it the vendor once the plugin's own list was due, before
// the request went. A new sign-in or new options build it again, once.
func TestFetchBuildsInfoOnce(t *testing.T) {
	sandbox(t)
	var mu sync.Mutex
	var told []string // x-plugin-model, as chat.headers set it, and x-models, as the loader was told them
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		told = append(told, r.Header.Get("x-plugin-model")+" "+r.Header.Get("x-models"))
		mu.Unlock()
		io.WriteString(w, "ok")
	}))
	defer srv.Close()
	t.Setenv("FAKE_BASE", srv.URL+"/v1")
	log := filepath.Join(t.TempDir(), "models.log")
	t.Setenv("FAKE_MODELS_LOG", log)
	asked := func() int {
		b, _ := os.ReadFile(log)
		return strings.Count(string(b), "asked")
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("testdata/fake/index.js")
	if _, err := Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := APIKey(ctx, "fakeco", 0, nil, "k1", NewAccount); err != nil {
		t.Fatal(err)
	}
	fetch := func() {
		t.Helper()
		res, err := Fetch(ctx, FetchRequest{Provider: "fakeco", Model: "fake-1", NPM: "@ai-sdk/openai-compatible",
			URL: srv.URL + "/v1/chat/completions", Method: "POST", Body: []byte(`{}`)})
		if err != nil {
			t.Fatal(err)
		}
		b, _ := io.ReadAll(res.Body)
		res.Body.Close()
		if string(b) != "ok" {
			t.Fatalf("answered %q", b)
		}
	}
	fetch()
	n := asked()
	for range 20 {
		fetch()
	}
	if got := asked() - n; got != 0 {
		t.Fatalf("20 requests asked the models hook %d more times, want 0", got)
	}
	mu.Lock()
	for _, m := range told {
		if m != "fake-1 fake-1,fake-claude,fake-gemini" {
			t.Fatalf("chat.headers and the loader were told %q, want fake-1 and every model", m)
		}
	}
	mu.Unlock()

	// a new sign-in for the account: built again, once
	if err := Restore(ctx, "fakeco", "fakeco", map[string]any{"type": "api", "key": "k2"}); err != nil {
		t.Fatal(err)
	}
	n = asked()
	for range 5 {
		fetch()
	}
	if got := asked() - n; got != 1 {
		t.Fatalf("after a new sign-in 5 requests asked the models hook %d times, want 1", got)
	}
	// new options: built again, once
	if err := SetProviderOptions(ctx); err != nil {
		t.Fatal(err)
	}
	n = asked()
	for range 5 {
		fetch()
	}
	if got := asked() - n; got != 1 {
		t.Fatalf("after new options 5 requests asked the models hook %d times, want 1", got)
	}
}
