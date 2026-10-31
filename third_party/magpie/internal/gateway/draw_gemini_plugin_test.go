package gateway

import (
	"encoding/base64"
	"fmt"
	"io"
	"net/http"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A plugin's model on Gemini's API (@ai-sdk/google) that gives images
// draws as a Google sign-in's image models do: on generateContent asking
// for images beside text, through the plugin's fetch, the picture read
// from its answer's inlineData (XYenon's Google AI Studio plugin). One that
// only says it gives images is asked for them too; an Imagen, which draws
// on :predict, and a model that gives only text are no drawers.
func TestGeminiPluginDraws(t *testing.T) {
	t.Setenv("FAKE_GEMINI_DRAWS", "1")
	var mu sync.Mutex
	var paths, bodies []string
	pid := besideFake(t, "google-ai-studio", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		mu.Lock()
		paths, bodies = append(paths, r.URL.RequestURI()), append(bodies, string(b))
		mu.Unlock()
		if !strings.Contains(r.URL.Path, ":streamGenerateContent") {
			w.WriteHeader(404)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		fmt.Fprintf(w, "data: %s\n\n", `{"candidates":[{"content":{"role":"model","parts":[{"text":"here"},{"inlineData":{"mimeType":"image/png","data":"`+base64.StdEncoding.EncodeToString(pngBytes)+`"}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1290}}`)
	}))
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	var drawers []string
	for _, m := range Drawers(*p) {
		drawers = append(drawers, m.ID)
	}
	slices.Sort(drawers)
	if !slices.Equal(drawers, []string{"gemini-3-flash-canvas", "gemini-3-pro-image-preview"}) {
		t.Fatalf("drawers %v, want gemini-3-flash-canvas and gemini-3-pro-image-preview", drawers)
	}
	s := New()
	for _, model := range drawers {
		mu.Lock()
		paths, bodies = nil, nil
		mu.Unlock()
		code, a, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"model":"`+pid+`/`+model+`","prompt":"a magpie"}`)
		if code != 200 || len(a.Data) != 1 {
			t.Fatalf("%s: %d %s", model, code, raw)
		}
		if b, _ := base64.StdEncoding.DecodeString(a.Data[0].B64); string(b) != string(pngBytes) {
			t.Fatalf("%s: image %q", model, b)
		}
		mu.Lock()
		if len(paths) != 1 || paths[0] != "/v1/models/"+model+":streamGenerateContent?alt=sse" {
			t.Fatalf("%s was asked at %v", model, paths)
		}
		if !strings.Contains(bodies[0], `"responseModalities":["TEXT","IMAGE"]`) || !strings.Contains(bodies[0], "a magpie") {
			t.Fatalf("%s was sent %s", model, bodies[0])
		}
		mu.Unlock()
	}
	// an edit: the picture goes as the prompt's inlineData
	mu.Lock()
	paths, bodies = nil, nil
	mu.Unlock()
	img := "data:image/png;base64," + base64.StdEncoding.EncodeToString(pngBytes)
	code, a, raw := postImages(t, s, "/v1/images/edits", "application/json", `{"model":"`+pid+`/gemini-3-flash-canvas","prompt":"bluer","images":[{"image_url":"`+img+`"}]}`)
	if code != 200 || len(a.Data) != 1 {
		t.Fatalf("edit: %d %s", code, raw)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(bodies) != 1 || !strings.Contains(bodies[0], `"inlineData"`) || !strings.Contains(bodies[0], base64.StdEncoding.EncodeToString(pngBytes)) {
		t.Fatalf("edit was sent %v", bodies)
	}
}
