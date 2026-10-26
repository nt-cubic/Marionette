package gateway

import (
	"encoding/json"
	"testing"
)

// chatImageDetails is the detail of every image_url in a Chat body, in
// order, "" for one without.
func chatImageDetails(t *testing.T, body []byte) []string {
	t.Helper()
	var v any
	if err := json.Unmarshal(body, &v); err != nil {
		t.Fatalf("body: %v\n%s", err, body)
	}
	var out []string
	var walk func(any)
	walk = func(v any) {
		switch x := v.(type) {
		case map[string]any:
			if x["type"] == "image_url" {
				u, _ := x["image_url"].(map[string]any)
				d, _ := u["detail"].(string)
				out = append(out, d)
			}
			for k, c := range x {
				if k != "image_url" {
					walk(c)
				}
			}
		case []any:
			for _, c := range x {
				walk(c)
			}
		}
	}
	walk(v)
	return out
}

// A Responses request's image detail reaches a Chat Completions upstream:
// Chat's image_url takes low, high and auto as they are, and Responses'
// original, which Chat lacks, goes as high, the closest it has. An image
// with no detail goes without one. A tool's image keeps its detail too
// (mysterious_dove, Codex: detail=high and detail=original were dropped).
func TestResponsesImageDetailReachesChat(t *testing.T) {
	img := `"image_url":"data:image/png;base64,` + pngA + `"`
	body := `{"model":"m","input":[
		{"role":"user","content":[{"type":"input_text","text":"look"},
			{"type":"input_image","detail":"high",` + img + `},
			{"type":"input_image","detail":"original",` + img + `},
			{"type":"input_image","detail":"low",` + img + `},
			{"type":"input_image","detail":"auto",` + img + `},
			{"type":"input_image",` + img + `}]},
		{"type":"function_call","call_id":"c1","name":"view_image","arguments":"{}"},
		{"type":"function_call_output","call_id":"c1","output":[{"type":"input_image","detail":"original",` + img + `}]}]}`
	r, err := parseResponses([]byte(body))
	if err != nil {
		t.Fatal(err)
	}
	out := buildChat(r, "m", "api.example.com", false)
	got := chatImageDetails(t, out)
	want := []string{"high", "high", "low", "auto", "", "high"}
	if len(got) != len(want) {
		t.Fatalf("want %d images, got %v\n%s", len(want), got, out)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("image %d: detail %q, want %q\n%s", i, got[i], want[i], out)
		}
	}
}

// A Chat request's image detail reaches a Responses upstream as the client
// asked, not as auto; one without detail still goes as auto (#1474).
func TestChatImageDetailReachesResponses(t *testing.T) {
	url := `"data:image/png;base64,` + pngA + `"`
	body := `{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"look"},
		{"type":"image_url","image_url":{"url":` + url + `,"detail":"low"}},
		{"type":"image_url","image_url":{"url":` + url + `,"detail":"high"}},
		{"type":"image_url","image_url":{"url":` + url + `}}]}]}`
	r, err := parseChat([]byte(body))
	if err != nil {
		t.Fatal(err)
	}
	out := buildResponses(r, "m", "api.example.com", false)
	images := inputImages(t, string(out))
	want := []string{"low", "high", "auto"}
	if len(images) != len(want) {
		t.Fatalf("want %d input_image, got %d\n%s", len(want), len(images), out)
	}
	for i, im := range images {
		if d, _ := im["detail"].(string); d != want[i] {
			t.Errorf("image %d: detail %q, want %q\n%s", i, d, want[i], out)
		}
	}
}
