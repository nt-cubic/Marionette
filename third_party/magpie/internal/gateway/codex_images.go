package gateway

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"fmt"
	"image"
	"image/draw"
	"image/gif"
	"image/jpeg"
	"image/png"
	"math"
	"strings"
	"sync"

	"github.com/tidwall/gjson"

	"github.com/yetone/magpie/internal/provider"
)

// Codex's own client sizes every inline image it sends to ChatGPT before
// sending it (openai/codex, codex-rs/utils/image and
// core/src/image_preparation.rs): one larger than its limits is scaled
// down with a triangle filter and written again in its own format — PNG
// as PNG, JPEG as JPEG at quality 85 — and a GIF as PNG; one that fits
// goes as it came. Another agent signed in to the same ChatGPT account
// through magpie sends its screenshots whole, and ChatGPT dropped
// ylorn's (Discord, v0.1.1179) once one took a turn past about 1 MiB.
// codexImagesFit does what Codex's client does, for a Codex account only.
//
// The limits are Codex's: detail auto, high or none fits 2048 px a side
// and 2500 patches of 32 px; "original" 6000 px and 10000 patches. Codex
// refuses "low" itself, and it is left as sent. WebP, which Go's standard
// library doesn't decode, is left as sent too.
func codexImagesFit(p provider.Provider, to provider.Protocol, body []byte) []byte {
	if p.Account == nil || p.Account.Agent != "codex" || to != provider.Responses || !bytes.Contains(body, []byte(`input_image`)) {
		return body
	}
	type swap struct {
		at, end int
		with    string
	}
	var swaps []swap
	var with []int // the input items with an image in them
	i := 0
	gjson.GetBytes(body, "input").ForEach(func(_, item gjson.Result) bool {
		if strings.Contains(item.Raw, "input_image") {
			with = append(with, i)
		}
		i++
		return true
	})
	for _, i := range with {
		for _, key := range []string{"content", "output"} {
			parts := gjson.GetBytes(body, fmt.Sprintf("input.%d.%s", i, key))
			if !parts.IsArray() {
				continue
			}
			for j := range parts.Array() {
				part := gjson.GetBytes(body, fmt.Sprintf("input.%d.%s.%d", i, key, j))
				if part.Get("type").String() != "input_image" {
					continue
				}
				url := gjson.GetBytes(body, fmt.Sprintf("input.%d.%s.%d.image_url", i, key, j))
				if url.Type != gjson.String || url.Index <= 0 || !strings.HasPrefix(url.Str, "data:") {
					continue
				}
				limits, ok := codexImageLimits(part.Get("detail").String())
				if !ok {
					continue
				}
				if fit, ok := codexImageFit(url.Str, limits); ok {
					swaps = append(swaps, swap{url.Index, url.Index + len(url.Raw), `"` + fit + `"`})
				}
			}
		}
	}
	if len(swaps) == 0 {
		return body
	}
	// the rest of the request goes byte for byte as it came, for the
	// prompt cache
	var out bytes.Buffer
	last := 0
	for _, s := range swaps {
		out.Write(body[last:s.at])
		out.WriteString(s.with)
		last = s.end
	}
	out.Write(body[last:])
	return out.Bytes()
}

type imageLimits struct{ side, patches int }

// codexPatch is the patch a Responses model reads an image in.
const codexPatch = 32

func codexImageLimits(detail string) (imageLimits, bool) {
	switch detail {
	case "", "auto", "high":
		return imageLimits{2048, 2500}, true
	case "original":
		return imageLimits{6000, 10000}, true
	}
	return imageLimits{}, false
}

func (l imageLimits) fit(w, h int) bool {
	pw, ph := (w+codexPatch-1)/codexPatch, (h+codexPatch-1)/codexPatch
	return w <= l.side && h <= l.side && pw*ph <= l.patches
}

// size is the size Codex's prompt_image_output_dimensions_for_limits
// gives an image of w by h: as it is when it fits; scaled to the longest
// side allowed; and then, if it still has too many patches, scaled by
// area with the patch grid rounded down.
func (l imageLimits) size(w, h int) (int, int) {
	w, h = max(w, 1), max(h, 1)
	if l.fit(w, h) {
		return w, h
	}
	scale := math.Min(float64(l.side)/float64(max(w, h)), 1)
	w = max(int(math.Round(float64(w)*scale)), 1)
	h = max(int(math.Round(float64(h)*scale)), 1)
	if l.fit(w, h) {
		return w, h
	}
	fw, fh := float64(w), float64(h)
	scale = math.Sqrt(codexPatch * codexPatch * float64(l.patches) / fw / fh)
	pw, ph := fw*scale/codexPatch, fh*scale/codexPatch
	scale *= math.Min(math.Floor(pw)/pw, math.Floor(ph)/ph)
	return max(int(math.Floor(fw*scale)), 1), max(int(math.Floor(fh*scale)), 1)
}

// codexImageMaxPixels keeps a request from decoding an image too large
// to hold: one past it goes as it came.
const codexImageMaxPixels = 80 << 20

// fitted holds what codexImageFit made of the images a conversation
// sends again on every turn.
var fitted = struct {
	sync.Mutex
	m map[[32]byte]string
}{m: map[[32]byte]string{}}

// codexImageFit is a data URL's image as Codex's client would send it,
// and whether that differs from what came.
func codexImageFit(url string, l imageLimits) (string, bool) {
	comma := strings.IndexByte(url, ',')
	if comma < 0 || !strings.HasSuffix(url[:comma], ";base64") {
		return "", false
	}
	key := sha256.Sum256([]byte(fmt.Sprint(l.side, l.patches, url)))
	fitted.Lock()
	got, ok := fitted.m[key]
	fitted.Unlock()
	if ok {
		return got, got != ""
	}
	got = codexImageRefit(url[comma+1:], l)
	fitted.Lock()
	if len(fitted.m) >= 64 {
		clear(fitted.m)
	}
	fitted.m[key] = got
	fitted.Unlock()
	return got, got != ""
}

// codexImageRefit is the data URL of an image's base64 resized and
// written again, or "" when it goes as it came.
func codexImageRefit(b64 string, l imageLimits) string {
	raw, err := base64.StdEncoding.DecodeString(b64)
	if err != nil {
		return ""
	}
	cfg, format, err := image.DecodeConfig(bytes.NewReader(raw))
	if err != nil || cfg.Width*cfg.Height > codexImageMaxPixels {
		return ""
	}
	w, h := l.size(cfg.Width, cfg.Height)
	if w == cfg.Width && h == cfg.Height && format != "gif" {
		return ""
	}
	var src image.Image
	switch format {
	case "png":
		src, err = png.Decode(bytes.NewReader(raw))
	case "jpeg":
		src, err = jpeg.Decode(bytes.NewReader(raw))
	case "gif":
		src, err = gif.Decode(bytes.NewReader(raw))
	default:
		return ""
	}
	if err != nil {
		return ""
	}
	if w != cfg.Width || h != cfg.Height {
		src = triangleResize(src, w, h)
	}
	var out bytes.Buffer
	mime := "image/png"
	if format == "jpeg" {
		mime = "image/jpeg"
		err = jpeg.Encode(&out, src, &jpeg.Options{Quality: 85})
	} else {
		err = png.Encode(&out, src)
	}
	if err != nil {
		return ""
	}
	return "data:" + mime + ";base64," + base64.StdEncoding.EncodeToString(out.Bytes())
}

// triangleResize scales src to w by h with a triangle (linear) filter
// widened by the scale when shrinking, as the image crate's
// FilterType::Triangle does, one axis and then the other.
func triangleResize(src image.Image, w, h int) *image.RGBA {
	b := src.Bounds()
	rgba, ok := src.(*image.RGBA)
	if !ok || rgba.Rect.Min != (image.Point{}) {
		rgba = image.NewRGBA(image.Rect(0, 0, b.Dx(), b.Dy()))
		draw.Draw(rgba, rgba.Rect, src, b.Min, draw.Src)
	}
	sw, sh := b.Dx(), b.Dy()
	// across: sw×sh → w×sh
	mid := make([]float32, w*sh*4)
	for x, k := range triangleWeights(sw, w) {
		for y := 0; y < sh; y++ {
			row := rgba.Pix[y*rgba.Stride:]
			var r, g, bl, a float32
			for i, wt := range k.w {
				p := row[(k.from+i)*4:]
				r += wt * float32(p[0])
				g += wt * float32(p[1])
				bl += wt * float32(p[2])
				a += wt * float32(p[3])
			}
			o := (y*w + x) * 4
			mid[o], mid[o+1], mid[o+2], mid[o+3] = r, g, bl, a
		}
	}
	// down: w×sh → w×h
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	for y, k := range triangleWeights(sh, h) {
		for x := 0; x < w; x++ {
			var c [4]float32
			for i, wt := range k.w {
				o := ((k.from+i)*w + x) * 4
				c[0] += wt * mid[o]
				c[1] += wt * mid[o+1]
				c[2] += wt * mid[o+2]
				c[3] += wt * mid[o+3]
			}
			o := y*dst.Stride + x*4
			for ch := range c {
				dst.Pix[o+ch] = uint8(min(max(math.Round(float64(c[ch])), 0), 255))
			}
			// premultiplied: no channel past its alpha
			for ch := 0; ch < 3; ch++ {
				dst.Pix[o+ch] = min(dst.Pix[o+ch], dst.Pix[o+3])
			}
		}
	}
	return dst
}

type tap struct {
	from int
	w    []float32
}

// triangleWeights are, for each of n outputs along an axis of src
// pixels, the source pixels it reads from and their weights.
func triangleWeights(src, n int) []tap {
	ratio := float64(src) / float64(n)
	scale := math.Max(ratio, 1)
	support := scale // the triangle's own support is 1
	taps := make([]tap, n)
	for i := range taps {
		center := (float64(i) + 0.5) * ratio
		from := max(int(math.Floor(center-support)), 0)
		to := min(int(math.Ceil(center+support)), src)
		var ws []float32
		var sum float64
		for j := from; j < to; j++ {
			wt := math.Max(1-math.Abs((float64(j)+0.5-center)/scale), 0)
			ws = append(ws, float32(wt))
			sum += wt
		}
		if sum == 0 {
			j := min(max(int(center), 0), src-1)
			taps[i] = tap{j, []float32{1}}
			continue
		}
		for k := range ws {
			ws[k] = float32(float64(ws[k]) / sum)
		}
		taps[i] = tap{from, ws}
	}
	return taps
}
