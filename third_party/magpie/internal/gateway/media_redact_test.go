package gateway

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http"
	"strings"
	"testing"

	"github.com/tidwall/gjson"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/redact"
	"github.com/yetone/magpie/internal/settings"
)

const mediaSecret = "sk-proj-abcdEFGH1234ijklMNOP5678qrst"
const mediaPrompt = "Draw \"Nightjar\"\non a phone: 13812345678, key: " + mediaSecret + ", gateway: acme-Zx9ab12cdEF"

func mediaRedactionOn(t *testing.T) {
	t.Helper()
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true, RedactWords: []string{"Nightjar"},
		RedactRules: []redact.Rule{{Kind: "GW_KEY", Prefix: "acme-"}}}); err != nil {
		t.Fatal(err)
	}
}

type mediaSent struct {
	path, prompt string
	image        []byte
}

// The vendor echoes its prompt in every supported reply shape. The
// captured request is inspected after the handler has completed.
func mediaEchoClient(t *testing.T, sent chan<- mediaSent, refuse bool) *http.Client {
	t.Helper()
	return &http.Client{Transport: countTransport(func(r *http.Request) (*http.Response, error) {
		got := mediaSent{path: r.URL.Path}
		if strings.HasPrefix(r.Header.Get("Content-Type"), "multipart/form-data") {
			if err := r.ParseMultipartForm(1 << 20); err != nil {
				return nil, err
			}
			defer r.MultipartForm.RemoveAll()
			got.prompt = r.FormValue("prompt")
			f, _, err := r.FormFile("image")
			if err != nil {
				return nil, err
			}
			defer f.Close()
			got.image, err = io.ReadAll(f)
			if err != nil {
				return nil, err
			}
		} else {
			b, err := io.ReadAll(r.Body)
			if err != nil {
				return nil, err
			}
			got.prompt = gjson.GetBytes(b, "prompt").String()
			if strings.HasSuffix(r.URL.Path, "/chat/completions") {
				got.prompt = gjson.GetBytes(b, "messages.0.content.0.text").String()
			} else if strings.HasSuffix(r.URL.Path, ":generateContent") {
				got.prompt = gjson.GetBytes(b, "contents.0.parts.0.text").String()
			}
			for _, path := range []string{"messages.0.content.1.image_url.url", "image.url", "images.0.image_url", "input_reference"} {
				if src := gjson.GetBytes(b, path).String(); strings.HasPrefix(src, "data:") {
					_, data, _ := strings.Cut(src, ",")
					got.image, err = base64.StdEncoding.DecodeString(data)
					if err != nil {
						return nil, err
					}
				}
			}
			if data := gjson.GetBytes(b, "contents.0.parts.1.inlineData.data").String(); data != "" {
				got.image, err = base64.StdEncoding.DecodeString(data)
				if err != nil {
					return nil, err
				}
			}
		}
		sent <- got
		png := base64.StdEncoding.EncodeToString(pngBytes)
		body := map[string]any{
			"data": []any{map[string]any{"b64_json": png, "revised_prompt": got.prompt}},
			"choices": []any{map[string]any{"message": map[string]any{"content": got.prompt,
				"images": []any{map[string]any{"image_url": map[string]string{"url": "data:image/png;base64," + png}}}}}},
			"candidates": []any{map[string]any{"content": map[string]any{"parts": []any{
				map[string]string{"text": got.prompt}, map[string]any{"inlineData": map[string]string{"mimeType": "image/png", "data": png}}}}}},
			"request_id": "req-1", "id": "video-1", "prompt": got.prompt,
		}
		code := 200
		if refuse {
			code, body = 400, map[string]any{"error": map[string]string{"message": "rejected: " + got.prompt}}
		}
		b, err := json.Marshal(body)
		return &http.Response{StatusCode: code, Header: http.Header{"Content-Type": {"application/json"}},
			Body: io.NopCloser(bytes.NewReader(b))}, err
	})}
}

func mediaInput(t *testing.T, model, field string, form bool) (string, string, []byte) {
	t.Helper()
	// Even bytes that look like secrets must reach the vendor unchanged.
	image := append([]byte(nil), pngBytes...)
	image = append(image, []byte(" Nightjar "+mediaSecret)...)
	if form {
		var buf bytes.Buffer
		mw := multipart.NewWriter(&buf)
		if err := mw.WriteField("model", model); err != nil {
			t.Fatal(err)
		}
		if err := mw.WriteField("prompt", mediaPrompt); err != nil {
			t.Fatal(err)
		}
		part, err := mw.CreateFormFile(field, "first.png")
		if err != nil {
			t.Fatal(err)
		}
		if _, err := part.Write(image); err != nil {
			t.Fatal(err)
		}
		if err := mw.Close(); err != nil {
			t.Fatal(err)
		}
		return mw.FormDataContentType(), buf.String(), image
	}
	obj := map[string]any{"model": model, "prompt": mediaPrompt}
	if field != "" {
		obj[field] = "data:image/png;base64," + base64.StdEncoding.EncodeToString(image)
	} else {
		image = nil
	}
	b, err := json.Marshal(obj)
	if err != nil {
		t.Fatal(err)
	}
	return "application/json", string(b), image
}

func assertMediaMasked(t *testing.T, prompt string) {
	t.Helper()
	for _, value := range []string{mediaSecret, "13812345678", "Nightjar", "acme-Zx9ab12cdEF"} {
		if strings.Contains(prompt, value) {
			t.Errorf("vendor received %q in prompt %q", value, prompt)
		}
	}
	for _, prefix := range []string{"{{API_KEY_", "{{PHONE_", "{{TERM_", "{{GW_KEY_"} {
		if !strings.Contains(prompt, prefix) {
			t.Errorf("vendor prompt %q has no %s placeholder", prompt, prefix)
		}
	}
}

func TestImagesRedacted(t *testing.T) {
	for _, tc := range []struct {
		name, model, path, upstream, reply string
		form, google, remote               bool
	}{
		{name: "images", model: "gpt-image-1", path: "/v1/images/generations", upstream: "/v1/images/generations", reply: "data.0.revised_prompt"},
		{name: "edit_json", model: "gpt-image-1", path: "/v1/images/edits", upstream: "/v1/images/edits", reply: "data.0.revised_prompt"},
		{name: "edit_multipart", model: "gpt-image-1", path: "/v1/images/edits", upstream: "/v1/images/edits", reply: "data.0.revised_prompt", form: true},
		{name: "chat", model: "painter-image-preview", path: "/v1/images/edits", upstream: "/v1/chat/completions", reply: "text"},
		{name: "gemini", model: "gemini-2.5-flash-image", path: "/v1/images/edits", upstream: "/v1beta/models/gemini-2.5-flash-image:generateContent", reply: "text", google: true},
		{name: "remote", model: "gpt-image-1", path: "/v1/images/generations", upstream: "/v1/images/generations", reply: "data.0.revised_prompt", remote: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			fresh(t)
			p := provider.Provider{ID: "art", Name: "Art", Chat: "https://art.test/v1", Key: "key", Models: []string{tc.model}}
			if tc.google {
				p.Chat = "https://generativelanguage.googleapis.com/v1beta/openai"
			}
			if tc.remote {
				p.Preset = provider.RemoteMagpiePreset
			}
			if err := provider.Save(p); err != nil {
				t.Fatal(err)
			}
			mediaRedactionOn(t)
			s := New()
			sent := make(chan mediaSent, 1)
			s.client = mediaEchoClient(t, sent, false)
			field := ""
			if strings.HasSuffix(tc.path, "/edits") {
				field = "image"
			}
			ct, body, image := mediaInput(t, "art/"+tc.model, field, tc.form)
			code, a, raw := postImages(t, s, tc.path, ct, body)
			if code != 200 || len(a.Data) != 1 {
				t.Fatalf("%d %s", code, raw)
			}
			got := <-sent
			if got.path != tc.upstream || !bytes.Equal(got.image, image) {
				t.Fatalf("vendor path/image: want %s / %q, got %s / %q", tc.upstream, image, got.path, got.image)
			}
			assertMediaMasked(t, got.prompt)
			if text := gjson.Get(raw, tc.reply).String(); text != mediaPrompt {
				t.Errorf("restored text = %q, want %q", text, mediaPrompt)
			}
			if b, _ := base64.StdEncoding.DecodeString(a.Data[0].B64); !bytes.Equal(b, pngBytes) {
				t.Errorf("returned image changed: %q", b)
			}
		})
	}
}

func TestVideosRedacted(t *testing.T) {
	for _, remote := range []bool{false, true} {
		for _, form := range []bool{false, true} {
			name := "grok_json"
			if remote {
				name = "remote_json"
			}
			if form {
				name = strings.TrimSuffix(name, "json") + "multipart"
			}
			t.Run(name, func(t *testing.T) {
				grokSignedIn(t)
				model, upstream := "grok/grok-imagine-video", "/v1/videos/generations"
				if remote {
					model, upstream = "office/grok-imagine-video", "/v1/videos"
					if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset,
						Chat: "https://office.test/v1", Key: "key", Models: []string{"grok-imagine-video"}}); err != nil {
						t.Fatal(err)
					}
				}
				mediaRedactionOn(t)
				s := New()
				sent := make(chan mediaSent, 1)
				s.client = mediaEchoClient(t, sent, false)
				ct, body, image := mediaInput(t, model, "input_reference", form)
				code, obj, raw, _ := request(t, s, "POST", "/v1/videos", ct, body)
				if code != 200 || obj["prompt"] != mediaPrompt {
					t.Fatalf("%d %s", code, raw)
				}
				got := <-sent
				if got.path != upstream || !bytes.Equal(got.image, image) {
					t.Fatalf("vendor path/image: want %s / %q, got %s / %q", upstream, image, got.path, got.image)
				}
				assertMediaMasked(t, got.prompt)
			})
		}
	}
}

func TestImagesRedactionSettings(t *testing.T) {
	for _, tc := range []struct {
		name, value, prefix string
		settings            settings.Settings
	}{
		{"off", mediaSecret, "", settings.Settings{}},
		{"secrets", mediaSecret, "{{API_KEY_", settings.Settings{Redact: true}},
		{"personal", "13812345678", "{{PHONE_", settings.Settings{RedactPersonal: true}},
		{"words", "Nightjar", "{{TERM_", settings.Settings{RedactWords: []string{"Nightjar"}}},
		{"rules", "acme-Zx9ab12cdEF", "{{GW_KEY_", settings.Settings{Redact: true, RedactRules: []redact.Rule{{Kind: "GW_KEY", Prefix: "acme-"}}}},
		{"rules_off", "acme-Zx9ab12cdEF", "", settings.Settings{RedactRules: []redact.Rule{{Kind: "GW_KEY", Prefix: "acme-"}}}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, e := easeled(t)
			if err := settings.Save(tc.settings); err != nil {
				t.Fatal(err)
			}
			code, _, raw := postImages(t, s, "/v1/images/generations", "application/json",
				string(mustJSON(map[string]string{"model": "art/gpt-image-1", "prompt": tc.value})))
			if code != 200 {
				t.Fatalf("%d %s", code, raw)
			}
			prompt := gjson.Get(e.got("/v1/images/generations")[0], "prompt").String()
			if tc.prefix == "" {
				if prompt != tc.value {
					t.Errorf("redaction off changed prompt: %q", prompt)
				}
			} else if strings.Contains(prompt, tc.value) || !strings.Contains(prompt, tc.prefix) {
				t.Errorf("vendor received %q, want %s placeholder", prompt, tc.prefix)
			}
		})
	}
}

func TestMediaRedactedErrors(t *testing.T) {
	for _, video := range []bool{false, true} {
		name, path, model := "images", "/v1/images/generations", "art/gpt-image-1"
		if video {
			name, path, model = "videos", "/v1/videos", "grok/grok-imagine-video"
		}
		t.Run(name, func(t *testing.T) {
			var s *Server
			if video {
				grokSignedIn(t)
				s = New()
			} else {
				s, _ = easeled(t)
			}
			mediaRedactionOn(t)
			sent := make(chan mediaSent, 1)
			s.client = mediaEchoClient(t, sent, true)
			ct, body, _ := mediaInput(t, model, "", false)
			code, obj, raw, _ := request(t, s, "POST", path, ct, body)
			if code != 400 || !strings.Contains(errorOf(obj), mediaPrompt) || strings.Contains(string(raw), "{{") {
				t.Fatalf("vendor's error was not restored: %d %s", code, raw)
			}
			assertMediaMasked(t, (<-sent).prompt)
		})
	}
}

func TestGrokVideoPollRedacted(t *testing.T) {
	grokSignedIn(t)
	up := newGrokMedia(t)
	mediaRedactionOn(t)
	s := New()
	ct, body, _ := mediaInput(t, "grok/grok-imagine-video", "", false)
	code, obj, raw, _ := request(t, s, "POST", "/v1/videos", ct, body)
	if code != 200 || obj["prompt"] != mediaPrompt {
		t.Fatalf("creation: %d %s", code, raw)
	}
	assertMediaMasked(t, gjson.Get(up.body("/v1/videos/generations", 0), "prompt").String())
	id, ok := obj["id"].(string)
	if !ok || id == "" {
		t.Fatalf("creation returned no id: %s", raw)
	}
	// Local Grok polls omit the creation prompt. Restoring responses must
	// leave their entire JSON unchanged, including the absence of prompt.
	for _, tc := range []struct {
		status   string
		progress int
		seconds  string
	}{{"queued", 0, ""}, {"in_progress", 40, ""}, {"completed", 100, "6"}} {
		want := map[string]any{"id": id, "object": "video", "created_at": obj["created_at"],
			"status": tc.status, "progress": tc.progress, "model": "grok/grok-imagine-video"}
		if tc.seconds != "" {
			want["seconds"] = tc.seconds
		}
		wantJSON := append(mustJSON(want), '\n')
		code, _, raw, head := request(t, s, "GET", "/v1/videos/"+id, "", "")
		if code != 200 || head.Get("Content-Type") != "application/json" || !bytes.Equal(raw, wantJSON) {
			t.Fatalf("%s poll: %d %s, want 200 %s", tc.status, code, raw, wantJSON)
		}
	}
}

func TestRemoteVideoPollRedacted(t *testing.T) {
	fresh(t)
	if err := provider.Save(provider.Provider{ID: "office", Name: "Office", Preset: provider.RemoteMagpiePreset,
		Chat: "https://office.test/v1", Key: "key", Models: []string{"grok-imagine-video"}}); err != nil {
		t.Fatal(err)
	}
	mediaRedactionOn(t)
	s := New()
	prompt := ""
	s.client = &http.Client{Transport: countTransport(func(r *http.Request) (*http.Response, error) {
		if r.Method == http.MethodPost {
			b, err := io.ReadAll(r.Body)
			if err != nil {
				return nil, err
			}
			prompt = gjson.GetBytes(b, "prompt").String()
		}
		b, err := json.Marshal(map[string]string{"id": "video-1", "model": "grok-imagine-video", "prompt": prompt, "status": "queued"})
		return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}},
			Body: io.NopCloser(bytes.NewReader(b))}, err
	})}
	ct, body, _ := mediaInput(t, "office/grok-imagine-video", "", false)
	code, obj, raw, _ := request(t, s, "POST", "/v1/videos", ct, body)
	if code != 200 || obj["prompt"] != mediaPrompt {
		t.Fatalf("creation: %d %s", code, raw)
	}
	assertMediaMasked(t, prompt)
	id, ok := obj["id"].(string)
	if !ok || id == "" {
		t.Fatalf("creation returned no id: %s", raw)
	}
	// A later poll has no prompt to mask, but the remote still echoes the
	// one it was sent when the video was created.
	code, obj, raw, _ = request(t, s, "GET", "/v1/videos/"+id, "", "")
	if code != 200 || obj["prompt"] != mediaPrompt {
		t.Fatalf("poll's prompt was not restored: %d %s", code, raw)
	}
}
