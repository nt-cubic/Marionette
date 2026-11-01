package gateway

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/plugin"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/testenv"
)

// A plugin's Gemini model (@ai-sdk/google) is tested as the gateway asks
// it: Gemini's generateContent, through the plugin's fetch. Test models
// sent it the body of the plugin's first other API, which Zed's cloud
// read as a Google request and answered 400 "failed to parse Google AI
// request: missing field contents" (Been on Discord: every Gemini model
// failed its test while agents were answered).
func TestPluginGeminiModelTestedOnGenerateContent(t *testing.T) {
	bun := testenv.Bun(t)
	fresh(t)
	t.Setenv("MAGPIE_BUN", bun)
	t.Cleanup(plugin.Settle)

	type seen struct{ path, body string }
	var mu sync.Mutex
	var got []seen
	up := testenv.FakeVendor(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		got = append(got, seen{r.URL.RequestURI(), string(b)})
		mu.Unlock()
		w.Header().Set("Content-Type", "text/event-stream")
		switch {
		case r.URL.Path == "/v1/chat/completions":
			fmt.Fprint(w, `data: {"id":"c","object":"chat.completion.chunk","model":"fake-1","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
		case strings.HasPrefix(r.URL.Path, "/v1/models/"):
			fmt.Fprint(w, `data: {"candidates":[{"content":{"role":"model","parts":[{"text":"hi from gemini"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2}}`+"\n\n")
		default:
			w.WriteHeader(404)
		}
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
	p, err := provider.Find("fakeco")
	if err != nil || !p.IsPlugin() {
		t.Fatalf("Find = %+v, %v", p, err)
	}

	res := p.TestModels(ctx, []string{"fake-gemini", "fake-1"})
	byModel := map[string]provider.Result{}
	for _, r := range res {
		byModel[r.Model] = r
	}
	if r := byModel["fake-gemini"]; !r.OK || r.Protocol != provider.CodeAssist {
		t.Errorf("fake-gemini's test: %+v; want it answered on Code Assist", r)
	}
	if r := byModel["fake-1"]; !r.OK || r.Protocol != provider.Chat {
		t.Errorf("fake-1's test: %+v; want it answered on chat, as before", r)
	}
	mu.Lock()
	defer mu.Unlock()
	var gemini []seen
	for _, s := range got {
		if strings.Contains(s.body, "fake-gemini") || strings.Contains(s.path, "fake-gemini") {
			gemini = append(gemini, s)
		}
	}
	if len(gemini) != 1 {
		t.Fatalf("fake-gemini reached the vendor %d times: %+v", len(gemini), got)
	}
	g := gemini[0]
	if !strings.HasPrefix(g.path, "/v1/models/fake-gemini:streamGenerateContent") || !strings.Contains(g.body, `"contents"`) || strings.Contains(g.body, `"request"`) {
		t.Errorf("fake-gemini's test reached the vendor as %+v; want generateContent's contents, outside Code Assist's envelope", g)
	}
}
