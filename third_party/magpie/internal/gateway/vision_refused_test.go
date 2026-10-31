package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// deepseekImageRefusal is DeepSeek's own 400 for an image sent to a model
// that takes text only, as a relay serving DeepSeek-V4.1-Flash passes it on.
const deepseekImageRefusal = `{"error":{"message":"Failed to deserialize the JSON body into the target type: messages[0]: unknown variant ` + "`image_url`" + `, expected ` + "`text`" + ` at line 1 column 162","type":"invalid_request_error","param":null,"code":"invalid_request_error"}}`

// sightUp is an upstream serving one model: blind turns away every
// request carrying an image with DeepSeek's 400; otherwise it answers ok.
// What it is sent is kept.
type sightUp struct {
	mu     sync.Mutex
	blind  bool
	bodies []string
}

func (u *sightUp) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	u.mu.Lock()
	u.bodies = append(u.bodies, string(body))
	u.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	if u.blind && strings.Contains(string(body), "image_url") {
		w.WriteHeader(400)
		io.WriteString(w, deepseekImageRefusal)
		return
	}
	if strings.Contains(string(body), `"stream":true`) {
		w.Header().Set("Content-Type", "text/event-stream")
		io.WriteString(w, `data: {"id":"x","choices":[{"index":0,"delta":{"role":"assistant","content":"ok"}}]}`+"\n\n")
		io.WriteString(w, `data: {"id":"x","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}`+"\n\n")
		io.WriteString(w, "data: [DONE]\n\n")
		return
	}
	io.WriteString(w, `{"id":"x","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}`)
}

func (u *sightUp) sent() []string {
	u.mu.Lock()
	defer u.mu.Unlock()
	return append([]string(nil), u.bodies...)
}

// sameModelTwice is xyenon's setup: DeepSeek-V4.1-Flash at two providers,
// pa's a relay that takes text only but whose list says nothing of images
// (models.dev says the model sees, so magpie counted it as seeing), pb's
// one that sees; pa falls back to pb. The Image recognition model is
// v/eye, unless vision is "" (magpie picks) or "off".
func sameModelTwice(t *testing.T, vision string) (s *Server, pa, pb *sightUp, eye *eyeUp) {
	t.Helper()
	fresh(t)
	pa, pb, eye = &sightUp{blind: true}, &sightUp{}, &eyeUp{}
	sa, sb, se := httptest.NewServer(pa), httptest.NewServer(pb), httptest.NewServer(eye)
	t.Cleanup(sa.Close)
	t.Cleanup(sb.Close)
	t.Cleanup(se.Close)
	const m = "DeepSeek-V4.1-Flash"
	save := func(p provider.Provider, ms []catalog.Model) {
		t.Helper()
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
		if err := catalog.SaveLive(p.ID, p.Chat, ms); err != nil {
			t.Fatal(err)
		}
	}
	save(provider.Provider{ID: "pa", Name: "PA", Chat: sa.URL + "/v1", Key: "k", Models: []string{m}, Fallback: []string{"pb/" + m}},
		[]catalog.Model{{ID: m, Images: true}}) // models.dev's word, nothing of the relay's own
	save(provider.Provider{ID: "pb", Name: "PB", Chat: sb.URL + "/v1", Key: "k", Models: []string{m}},
		[]catalog.Model{{ID: m, Images: true, ImageInput: imageInputBool(true)}})
	save(provider.Provider{ID: "v", Name: "V", Chat: se.URL + "/v1", Key: "k", Models: []string{"eye"}},
		[]catalog.Model{{ID: "eye", Images: true, ImageInput: imageInputBool(true)}})
	st := settings.Load()
	st.Vision = vision
	if err := settings.Save(st); err != nil {
		t.Fatal(err)
	}
	return New(), pa, pb, eye
}

// imageTurns are a request with an image on each of the three APIs.
func imageTurns(model string) []struct{ path, body string } {
	return []struct{ path, body string }{
		{"/v1/chat/completions", `{"model":"` + model + `","messages":[{"role":"user","content":[{"type":"text","text":"read"},{"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}]}]}`},
		{"/v1/responses", `{"model":"` + model + `","input":[{"role":"user","content":[{"type":"input_text","text":"read"},{"type":"input_image","image_url":"data:image/png;base64,aGVsbG8="}]}]}`},
		{"/v1/messages", `{"model":"` + model + `","max_tokens":16,"messages":[{"role":"user","content":[{"type":"text","text":"read"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}]}]}`},
	}
}

// The model the user picked, at the provider they picked, is given the
// image described by the Image recognition model they set, when it turns
// the image away: not the same model at another provider that sees, which
// its fallback or its group reaches (xyenon on Discord). After the first
// refusal it is described up front.
func TestPickedModelThatRefusesAnImageIsGivenItDescribed(t *testing.T) {
	for _, asked := range []string{"pa/DeepSeek-V4.1-Flash", "group/g", "group/auto-deepseek-v4-1-flash"} {
		for _, tc := range imageTurns(asked) {
			t.Run(asked+tc.path, func(t *testing.T) {
				s, pa, pb, eye := sameModelTwice(t, "v/eye")
				if asked == "group/g" {
					if err := provider.SaveGroup(provider.Group{ID: "g", Name: "G", Members: []string{"pa/DeepSeek-V4.1-Flash", "pb/DeepSeek-V4.1-Flash"}, Routing: provider.Ordered}); err != nil {
						t.Fatal(err)
					}
				}
				for turn := range 2 {
					rec := httptest.NewRecorder()
					s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body)))
					if rec.Code != 200 {
						t.Fatalf("turn %d: %d %s", turn, rec.Code, rec.Body.String())
					}
				}
				if got := pb.sent(); len(got) != 0 {
					t.Fatalf("the same model at the other provider was asked %d times: %s", len(got), got[0])
				}
				got := pa.sent()
				// the first turn: the image, refused, then the description;
				// the second turn: the description at once
				if len(got) != 3 {
					t.Fatalf("the picked provider was asked %d times, want 3: %q", len(got), got)
				}
				for _, b := range got[1:] {
					if !strings.Contains(b, "HELLO") || !strings.Contains(b, "v/eye describes it") || strings.Contains(b, "aGVsbG8=") {
						t.Fatalf("the picked provider was sent %s", b)
					}
				}
				if n := len(eye.sent("eye")); n != 1 {
					t.Fatalf("the image was described %d times, want 1", n)
				}
			})
		}
	}
}

// With nothing to describe images (Image recognition off), the same model
// at a provider that sees is the only way to answer: the fallback is
// asked, as before.
func TestRefusedImageWithNoDescriberGoesToOneThatSees(t *testing.T) {
	s, pa, pb, _ := sameModelTwice(t, "off")
	tc := imageTurns("pa/DeepSeek-V4.1-Flash")[0]
	for turn := range 2 {
		rec := httptest.NewRecorder()
		s.Handler().ServeHTTP(rec, httptest.NewRequest("POST", tc.path, strings.NewReader(tc.body)))
		if rec.Code != 200 {
			t.Fatalf("turn %d: %d %s", turn, rec.Code, rec.Body.String())
		}
	}
	if n := len(pb.sent()); n != 2 {
		t.Fatalf("the provider that sees was asked %d times, want 2", n)
	}
	if len(pa.sent()) == 0 {
		t.Fatal("the picked provider was never asked")
	}
}

func TestImageRefused(t *testing.T) {
	yes := []string{
		deepseekImageRefusal,
		`{"error":{"message":"Failed to deserialize the JSON body into the target type: input[0].content[1]: unknown variant ` + "`input_image`" + `"}}`,
		`{"error":{"message":"Invalid content type. image_url is only supported by certain models.","type":"invalid_request_error"}}`,
		`{"error":{"message":"No endpoints found that support image input","code":404}}`,
		`{"object":"error","message":"Qwen3-32B is not a multimodal model","type":"BadRequestError"}`,
		`{"error":{"message":"This model does not support image input"}}`,
		`{"error":{"message":"Image input is not supported for this model"}}`,
		`{"error":{"message":"当前模型不支持图片输入"}}`,
	}
	for _, b := range yes {
		if !imageRefused(400, []byte(b)) {
			t.Errorf("not read as an image refused: %s", b)
		}
	}
	no := []string{
		`{"error":{"message":"Failed to deserialize the JSON body into the target type: tools[0]: unknown variant ` + "`custom`" + `"}}`,
		`{"error":{"message":"You exceeded your current quota"}}`,
		`{"error":{"message":"prompt is too long"}}`,
		`{"error":{"message":"Insufficient balance"}}`,
	}
	for _, b := range no {
		if imageRefused(400, []byte(b)) {
			t.Errorf("read as an image refused: %s", b)
		}
	}
	if imageRefused(429, []byte(deepseekImageRefusal)) || imageRefused(500, []byte(deepseekImageRefusal)) {
		t.Error("a 429 or 500 read as an image refused")
	}
}
