package gateway

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

const unmaskedKey = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"

// isee1_1 on Discord: a plugin's provider (a translation plugin there)
// takes "Send requests unmasked" as any provider does. Set, the plugin's
// fetch gets the request as the agent wrote it, with Settings' redaction
// on; unset, or able to fall back to a vendor that isn't set, it is masked.
func TestPluginProviderUnmaskedSwitch(t *testing.T) {
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)
	var mu sync.Mutex
	var got string
	up := testenv.FakeVendor(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = string(b)
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	}))
	defer up.Close()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	abs, _ := filepath.Abs("../plugin/testdata/fake/index.js")
	if _, err := plugin.Add(ctx, abs); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.Providers(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := plugin.APIKey(ctx, "fakeco", 0, nil, "k1", plugin.NewAccount); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true}); err != nil {
		t.Fatal(err)
	}
	vendor := provider.Provider{ID: "vendor", Name: "Vendor", Key: "k", Models: []string{"m9"}, Anthropic: "https://api.vendor.example"}
	if err := provider.Save(vendor); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, tc := range []struct {
		name     string
		on       bool
		fallback []string
		whole    bool
	}{
		{name: "set", on: true, whole: true},
		{name: "unset"},
		{name: "set, falls back to a vendor", on: true, fallback: []string{"vendor/m9"}},
	} {
		p, err := provider.Find("fakeco")
		if err != nil || !p.IsPlugin() {
			t.Fatalf("%s: Find = %+v, %v", tc.name, p, err)
		}
		p.Unredacted, p.Fallback = tc.on, tc.fallback
		if err := provider.Save(*p); err != nil {
			t.Fatal(err)
		}
		if q, _ := provider.Find("fakeco"); q == nil || q.Unredacted != tc.on {
			t.Fatalf("%s: saved as %+v", tc.name, q)
		}
		code, body := postAs(t, s, "", `{"model":"fakeco/fake-1","messages":[{"role":"user","content":"my key is `+unmaskedKey+`"}]}`)
		if code != 200 {
			t.Fatalf("%s: %d %s", tc.name, code, body)
		}
		mu.Lock()
		sent := got
		mu.Unlock()
		if whole := strings.Contains(sent, unmaskedKey); whole != tc.whole {
			t.Fatalf("%s: sent whole %v, want %v: %s", tc.name, whole, tc.whole, sent)
		}
	}
}

// A turn relayed on Codex's own ChatGPT sign-in goes as Codex wrote it
// when the user set the Codex subscription to go unmasked, and masked
// when not.
func TestCodexSignInUnmaskedSwitch(t *testing.T) {
	for _, on := range []bool{true, false} {
		t.Run(map[bool]string{true: "set", false: "unset"}[on], func(t *testing.T) { codexUnmasked(t, on) })
	}
}

func codexUnmasked(t *testing.T, on bool) {
	{
		codexSignedIn(t)
		var got []byte
		chatgptBase(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			got, _ = io.ReadAll(r.Body)
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, sse(
				`data: {"type":"response.created","response":{"id":"r1","model":"gpt-5.5"}}`,
				`data: {"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":7,"output_tokens":1}}}`))
		}))
		if err := settings.Save(settings.Settings{Redact: true}); err != nil {
			t.Fatal(err)
		}
		p, err := provider.Find("codex")
		if err != nil || p.Account == nil {
			t.Fatalf("Find(codex) = %+v, %v", p, err)
		}
		p.Unredacted = on
		if err := provider.Save(*p); err != nil {
			t.Fatal(err)
		}
		code, body := codexPost(t, `{"model":"gpt-5.5","stream":true,"input":"my key is `+unmaskedKey+`"}`)
		if code != 200 {
			t.Fatalf("%d %s", code, body)
		}
		if whole := strings.Contains(string(got), unmaskedKey); whole != on {
			t.Fatalf("sent whole %v, set %v: %s", whole, on, got)
		}
	}
}

// chatgptBase stands the ChatGPT backend in with h for this test.
func chatgptBase(t *testing.T, h http.Handler) {
	t.Helper()
	up := httptest.NewServer(h)
	t.Cleanup(up.Close)
	was := provider.CodexBase
	provider.CodexBase = up.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })
}
