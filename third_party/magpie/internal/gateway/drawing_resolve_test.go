package gateway

import (
	"encoding/base64"
	"fmt"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

func TestDrawingResolvesLiveImageModels(t *testing.T) {
	fresh(t)
	e := &easel{}
	up := httptest.NewServer(e)
	defer up.Close()
	p := provider.Provider{ID: "relay", Name: "Relay", Responses: up.URL + "/v1", Key: "key"}
	if err := provider.Save(p); err != nil {
		t.Fatal(err)
	}
	if err := catalog.SaveLive(p.ID, p.Responses, []catalog.Model{
		{ID: "text"}, {ID: "gpt-image-2", Draws: true}, {ID: "gpt-image-2.5-flare", Draws: true},
	}); err != nil {
		t.Fatal(err)
	}
	for _, entry := range provider.Catalog() {
		if strings.Contains(entry.Model, "gpt-image") {
			t.Fatalf("image model in chat catalog: %s", entry.ID)
		}
	}
	st := settings.Load()
	st.ImageGen = "relay/gpt-image-2.5-flare"
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
	s := New()
	for _, model := range []string{"gpt-image-2", "gpt-image-2.5-flare", "relay/gpt-image-2"} {
		t.Run(model, func(t *testing.T) {
			code, answer, raw := postImages(t, s, "/v1/images/generations", "application/json",
				fmt.Sprintf(`{"model":%q,"prompt":"a magpie"}`, model))
			upstreamModel := strings.TrimPrefix(model, "relay/")
			if code != 200 || answer.Model != "relay/"+upstreamModel {
				t.Fatalf("%d %s", code, raw)
			}
			requests := e.got("/v1/images/generations")
			if !strings.Contains(requests[len(requests)-1], fmt.Sprintf(`"model":%q`, upstreamModel)) {
				t.Fatalf("wrong upstream model: %v", requests)
			}
		})
	}
	code, answer, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"prompt":"a magpie"}`)
	if code != 200 || answer.Model != st.ImageGen {
		t.Fatalf("default: %d %s", code, raw)
	}
	before := len(e.got("/v1/images/generations"))
	code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"model":"gpt-image-missing","prompt":"a magpie"}`)
	if code != 404 || len(e.got("/v1/images/generations")) != before {
		t.Fatalf("unknown model: %d %s", code, raw)
	}
	st.ImageGen = "off"
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
	code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"model":"gpt-image-2","prompt":"a magpie"}`)
	if code != 200 {
		t.Fatalf("explicit model with default off: %d %s", code, raw)
	}
	code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"prompt":"a magpie"}`)
	if code != 400 {
		t.Fatalf("default off: %d %s", code, raw)
	}
	image := "data:image/png;base64," + base64.StdEncoding.EncodeToString(pngBytes)
	code, _, raw = postImages(t, s, "/v1/images/edits", "application/json",
		fmt.Sprintf(`{"model":"gpt-image-2","prompt":"a blue magpie","image":%q}`, image))
	if code != 200 || len(e.got("/v1/images/edits")) != 1 || len(e.got("/v1/chat/completions")) != 0 {
		t.Fatalf("edit: %d %s", code, raw)
	}
	e.mu.Lock()
	contentType := e.ct["/v1/images/edits"]
	body := e.sent["/v1/images/edits"][0]
	e.mu.Unlock()
	request := httptest.NewRequest(http.MethodPost, "/", strings.NewReader(body))
	request.Header.Set("Content-Type", contentType)
	if err := request.ParseMultipartForm(1 << 20); err != nil {
		t.Fatal(err)
	}
	defer request.MultipartForm.RemoveAll()
	if request.FormValue("model") != "gpt-image-2" || len(request.MultipartForm.File["image"]) != 1 {
		t.Fatalf("edit model or image lost: %v", request.MultipartForm)
	}
	var editBody strings.Builder
	writer := multipart.NewWriter(&editBody)
	if err := writer.WriteField("model", "gpt-image-2.5-flare"); err != nil {
		t.Fatal(err)
	}
	if err := writer.WriteField("prompt", "a blue magpie"); err != nil {
		t.Fatal(err)
	}
	part, err := writer.CreateFormFile("image", "input.png")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := part.Write(pngBytes); err != nil {
		t.Fatal(err)
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	code, _, raw = postImages(t, s, "/v1/images/edits", writer.FormDataContentType(), editBody.String())
	if code != 200 || len(e.got("/v1/images/edits")) != 2 {
		t.Fatalf("multipart edit: %d %s", code, raw)
	}
}

func TestDrawingResolutionUsesEnabledProvidersInOrder(t *testing.T) {
	fresh(t)
	for _, p := range []provider.Provider{
		{ID: "off", Name: "Off", Responses: "https://off.example/v1", Key: "key", Off: true},
		{ID: "first", Name: "First", Responses: "https://first.example/v1", Key: "key"},
		{ID: "second", Name: "Second", Responses: "https://second.example/v1", Key: "key"},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
		if err := catalog.SaveLive(p.ID, p.Responses, []catalog.Model{{ID: "gpt-image-2", Draws: true}}); err != nil {
			t.Fatal(err)
		}
	}
	for _, test := range []struct{ requested, provider, model string }{
		{"gpt-image-2", "first", "gpt-image-2"},
		{"second/gpt-image-2", "second", "gpt-image-2"},
		{"second/gpt-image-new", "second", "gpt-image-new"},
	} {
		p, model, ok := resolveDrawing(test.requested)
		if !ok || p.ID != test.provider || model != test.model {
			t.Fatalf("resolve %q = %s/%s, %v", test.requested, p.ID, model, ok)
		}
	}
	if _, _, ok := resolveDrawing("off/gpt-image-2"); ok {
		t.Fatal("resolved disabled provider")
	}
}
