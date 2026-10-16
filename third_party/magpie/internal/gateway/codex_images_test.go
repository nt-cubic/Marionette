package gateway

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"io"
	"math/rand/v2"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
)

// The sizes Codex's own client gives images, from openai/codex's tests
// (codex-rs/utils/image/src/image_tests.rs) and a MacBook's full-screen
// screenshot as ylorn's would be.
func TestCodexImageSizeAsCodex(t *testing.T) {
	high, original := imageLimits{2048, 2500}, imageLimits{6000, 10000}
	for _, c := range []struct {
		l            imageLimits
		w, h, ow, oh int
	}{
		{high, 64, 32, 64, 32},
		{high, 2048, 2048, 1600, 1600},
		{high, 2050, 2, 2048, 2},
		{original, 6401, 100, 6000, 94},
		{original, 4096, 2048, 4096, 2048},
		{high, 3456, 2234, 1979, 1280},
	} {
		if w, h := c.l.size(c.w, c.h); w != c.ow || h != c.oh {
			t.Errorf("%v %dx%d → %dx%d, Codex gives %dx%d", c.l, c.w, c.h, w, h, c.ow, c.oh)
		}
	}
}

// screenshot is a w×h image: black on its left half, white on its right,
// with a red band 64 px tall across the middle — what has to stay where
// it was for the model to read it.
func shotImage(w, h int) *image.RGBA {
	m := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			c := color.RGBA{255, 255, 255, 255}
			if x < w/2 {
				c = color.RGBA{0, 0, 0, 255}
			}
			if y >= h/2-32 && y < h/2+32 {
				c = color.RGBA{220, 20, 20, 255}
			}
			m.SetRGBA(x, y, c)
		}
	}
	return m
}

func shotURL(t *testing.T, m image.Image, format string) string {
	t.Helper()
	var b bytes.Buffer
	if format == "jpeg" {
		jpeg.Encode(&b, m, &jpeg.Options{Quality: 95})
	} else {
		png.Encode(&b, m)
	}
	return "data:image/" + format + ";base64," + base64.StdEncoding.EncodeToString(b.Bytes())
}

func shotDecode(t *testing.T, url string) (image.Image, string) {
	t.Helper()
	comma := strings.IndexByte(url, ',')
	raw, err := base64.StdEncoding.DecodeString(url[comma+1:])
	if err != nil {
		t.Fatalf("not base64: %v", err)
	}
	m, format, err := image.Decode(bytes.NewReader(raw))
	if err != nil {
		t.Fatalf("not an image: %v", err)
	}
	return m, format
}

// legible: the halves and the band are where they were, in their colours.
func shotLegible(t *testing.T, m image.Image) {
	t.Helper()
	b := m.Bounds()
	w, h := b.Dx(), b.Dy()
	at := func(x, y int) (uint32, uint32, uint32) {
		r, g, bl, _ := m.At(x, y).RGBA()
		return r >> 8, g >> 8, bl >> 8
	}
	if r, g, bl := at(w/4, h/4); r > 16 || g > 16 || bl > 16 {
		t.Errorf("left half isn't black: %d %d %d", r, g, bl)
	}
	if r, g, bl := at(3*w/4, 3*h/4); r < 240 || g < 240 || bl < 240 {
		t.Errorf("right half isn't white: %d %d %d", r, g, bl)
	}
	if r, g, bl := at(w/4, h/2); r < 190 || g > 50 || bl > 50 {
		t.Errorf("the band isn't red: %d %d %d", r, g, bl)
	}
}

func codexAccount() provider.Provider {
	return provider.Provider{ID: "codex", Account: &provider.Account{Agent: "codex", User: "me@example.com"}}
}

func responsesWith(parts ...map[string]any) []byte {
	b, _ := json.Marshal(map[string]any{"model": "gpt-5.5", "stream": true, "input": []any{
		map[string]any{"type": "message", "role": "user", "content": append([]any{
			map[string]any{"type": "input_text", "text": "what does this show?"}}, anys(parts)...)},
		map[string]any{"type": "function_call_output", "call_id": "c1", "output": []any{
			map[string]any{"type": "input_text", "text": "the page"}}},
	}})
	return b
}

func anys(parts []map[string]any) []any {
	out := make([]any, len(parts))
	for i, p := range parts {
		out[i] = p
	}
	return out
}

// A screenshot larger than Codex's own client sends goes to a ChatGPT
// account at the size Codex would send it, as a PNG still, and the
// picture in it is the same picture. The rest of the request is the same
// bytes, for the prompt cache.
func TestCodexScreenshotSizedAsCodex(t *testing.T) {
	big := shotURL(t, shotImage(3456, 2234), "png")
	body := responsesWith(map[string]any{"type": "input_image", "image_url": big})
	out := codexImagesFit(codexAccount(), provider.Responses, body)

	url := gjson.GetBytes(out, "input.0.content.1.image_url").String()
	if !strings.HasPrefix(url, "data:image/png;base64,") {
		t.Fatalf("not a PNG: %.40s", url)
	}
	m, format := shotDecode(t, url)
	if format != "png" || m.Bounds().Dx() != 1979 || m.Bounds().Dy() != 1280 {
		t.Fatalf("%s %v, Codex sends 1979x1280 png", format, m.Bounds())
	}
	shotLegible(t, m)
	if len(out) >= len(body) {
		t.Errorf("%d bytes after, %d before", len(out), len(body))
	}
	if string(bytes.Replace(out, []byte(url), []byte("IMG"), 1)) != string(bytes.Replace(body, []byte(big), []byte("IMG"), 1)) {
		t.Errorf("the rest of the request changed")
	}
}

// A JPEG stays a JPEG, at Codex's quality; an image in a tool's output is
// sized too; "original" detail has Codex's larger budget.
func TestCodexImagesSizedInEveryPlace(t *testing.T) {
	photo := shotURL(t, shotImage(3000, 3000), "jpeg")
	wide := shotURL(t, shotImage(6401, 100), "png")
	body := responsesWith(
		map[string]any{"type": "input_image", "image_url": photo, "detail": "high"},
		map[string]any{"type": "input_image", "image_url": wide, "detail": "original"},
	)
	body = []byte(strings.Replace(string(body), `{"text":"the page","type":"input_text"}`,
		`{"text":"the page","type":"input_text"},{"type":"input_image","image_url":"`+photo+`"}`, 1))
	out := codexImagesFit(codexAccount(), provider.Responses, body)

	for path, want := range map[string][3]any{
		"input.0.content.1.image_url": {"jpeg", 1600, 1600},
		"input.0.content.2.image_url": {"png", 6000, 94},
		"input.1.output.1.image_url":  {"jpeg", 1600, 1600},
	} {
		url := gjson.GetBytes(out, path).String()
		if url == "" {
			t.Fatalf("%s gone: %.300s", path, out)
		}
		m, format := shotDecode(t, url)
		if format != want[0] || m.Bounds().Dx() != want[1] || m.Bounds().Dy() != want[2] {
			t.Errorf("%s: %s %v, want %v", path, format, m.Bounds(), want)
		}
		if want[1] == 1600 {
			shotLegible(t, m)
		}
	}
}

// What Codex's client sends as it came goes as it came: an image that
// fits, one with "low" detail (Codex refuses that itself), and anything
// that isn't an image a decoder reads.
func TestCodexImagesThatFitGoAsTheyCame(t *testing.T) {
	small := shotURL(t, shotImage(64, 32), "png")
	big := shotURL(t, shotImage(3000, 3000), "png")
	body := responsesWith(
		map[string]any{"type": "input_image", "image_url": small},
		map[string]any{"type": "input_image", "image_url": big, "detail": "low"},
		map[string]any{"type": "input_image", "image_url": "data:image/png;base64,bm90IGFuIGltYWdl"},
		map[string]any{"type": "input_image", "image_url": "https://example.invalid/a.png"},
	)
	if out := codexImagesFit(codexAccount(), provider.Responses, body); !bytes.Equal(out, body) {
		t.Errorf("changed")
	}
}

// Only a ChatGPT account's requests are sized: another vendor, an API
// key, or another protocol is sent the image as the agent sent it.
func TestCodexImagesOnlyForChatGPT(t *testing.T) {
	body := responsesWith(map[string]any{"type": "input_image", "image_url": shotURL(t, shotImage(3456, 2234), "png")})
	for _, c := range []struct {
		p  provider.Provider
		to provider.Protocol
	}{
		{provider.Provider{ID: "openai"}, provider.Responses},
		{provider.Provider{ID: "claude", Account: &provider.Account{Agent: "claude"}}, provider.Responses},
		{codexAccount(), provider.Chat},
	} {
		if out := codexImagesFit(c.p, c.to, body); !bytes.Equal(out, body) {
			t.Errorf("%s %v: changed", c.p.ID, c.to)
		}
	}
}

// An agent other than Codex — Pi's openai-codex, as ylorn's — asking a
// ChatGPT account through magpie: the backend gets the screenshot at
// Codex's size, which takes the turn from past 1 MiB to under it.
func TestCodexAccountGetsScreenshotAtCodexSize(t *testing.T) {
	codexSignedIn(t, "spare@example.com")
	var got []byte
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got, _ = io.ReadAll(r.Body)
		io.WriteString(w, sse(
			`data: {"type":"response.created","response":{"id":"r1","model":"gpt-5.5"}}`,
			`data: {"type":"response.output_text.delta","delta":"a screenshot"}`,
			`data: {"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":7,"output_tokens":2}}}`))
	}))
	t.Cleanup(up.Close)
	was := provider.CodexBase
	provider.CodexBase = up.URL + "/backend-api/codex"
	t.Cleanup(func() { provider.CodexBase = was })

	// a photo along its top, which no PNG makes small: the turn past
	// 1 MiB, as ylorn's was
	shot := shotImage(3456, 2234)
	noise := rand.New(rand.NewPCG(1, 2))
	for i := 0; i < 160*shot.Stride; i++ {
		if i%4 != 3 {
			shot.Pix[i] = byte(noise.Uint32())
		}
	}
	body := string(responsesWith(map[string]any{"type": "input_image", "image_url": shotURL(t, shot, "png")}))
	body = strings.Replace(body, `"model":"gpt-5.5"`, `"model":"codex/gpt-5.5"`, 1)
	if len(body) < 1<<20 {
		t.Fatalf("the turn is %d bytes, not past 1 MiB", len(body))
	}
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("POST", "/v1/responses", strings.NewReader(body)))
	if rec.Code != 200 || got == nil {
		t.Fatalf("status %d: %.300s", rec.Code, rec.Body.String())
	}
	m, _ := shotDecode(t, gjson.GetBytes(got, "input.0.content.1.image_url").String())
	if m.Bounds().Dx() != 1979 || m.Bounds().Dy() != 1280 {
		t.Errorf("sent %v, Codex sends 1979x1280", m.Bounds())
	}
	t.Logf("%d bytes from the agent, %d to ChatGPT", len(body), len(got))
}
