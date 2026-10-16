package gateway

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/provider"
)

// wbStudio is WorkBuddy's API as far as drawing goes: its product config,
// which lists image models by tag among its chat models, and its images
// API, which answers OpenAI's shape inside WorkBuddy's {code, msg, data}.
type wbStudio struct {
	models string // the config's models

	mu     sync.Mutex
	asked  []string // method and path
	bodies map[string]map[string]any
	heads  map[string]http.Header
	answer func(path string, body map[string]any) string
}

func (s *wbStudio) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	b, _ := io.ReadAll(r.Body)
	var body map[string]any
	json.Unmarshal(b, &body)
	s.mu.Lock()
	s.asked = append(s.asked, r.Method+" "+r.URL.Path)
	if s.bodies == nil {
		s.bodies, s.heads = map[string]map[string]any{}, map[string]http.Header{}
	}
	s.bodies[r.URL.Path], s.heads[r.URL.Path] = body, r.Header.Clone()
	s.mu.Unlock()
	w.Header().Set("Content-Type", "application/json")
	if r.URL.Path == "/v3/config" {
		io.WriteString(w, `{"code":0,"msg":"OK","data":{"agents":[{"name":"cli","models":["hy3"]}],"models":`+s.models+`}}`)
		return
	}
	io.WriteString(w, s.answer(r.URL.Path, body))
}

func (s *wbStudio) got(path string) (map[string]any, http.Header) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.bodies[path], s.heads[path]
}

func (s *wbStudio) count(what string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	n := 0
	for _, a := range s.asked {
		if a == what {
			n++
		}
	}
	return n
}

const wbHunyuanModels = `[{"id":"hy3","name":"Hy3","tags":[]},
	{"id":"hunyuan-image-alpha","name":"Hunyuan Image Alpha","tags":["text-to-image"]},
	{"id":"hunyuan-image-alpha-edit","name":"Hunyuan Image Alpha Edit","tags":["image-to-image"]},
	{"id":"hunyuan-image-v3.0-art","name":"Hunyuan-Image-v3.0-art","tags":["text-to-image"]}]`

const wbGPTImageModels = `[{"id":"gpt-5.5","name":"GPT-5.5"},
	{"id":"gpt-image-2.5-sunburst","name":"GPT-Image-2.5-Sunburst","tags":["text-to-image","image-to-image"]},
	{"id":"seedance-2.5","name":"Seedance-2.5","tags":["text-to-video","image-to-video"]}]`

// wbSignedIn signs WorkBuddy's desktop app in to the build authID names
// ("workbuddy-desktop", "workbuddy-desktop-ai"), its API at the studio, and
// gives magpie's provider for it.
func wbSignedIn(t *testing.T, id, authID string, st http.Handler) provider.Provider {
	t.Helper()
	fresh(t)
	wbImages.Lock()
	wbImages.m = map[string]*wbImageList{}
	wbImages.Unlock()
	home := os.Getenv("HOME")
	base := filepath.Join(home, ".local", "share")
	switch runtime.GOOS {
	case "darwin":
		base = filepath.Join(home, "Library", "Application Support")
	case "windows":
		base = filepath.Join(home, "AppData", "Local")
	}
	future := strconv.FormatInt(time.Now().Add(24*time.Hour).UnixMilli(), 10)
	info := `{"account":{"uid":"u1","nickname":"me"},"auth":{"accessToken":"tok","refreshToken":"r","expiresAt":` + future + `,"refreshExpiresAt":` + future + `}}`
	auth := filepath.Join(base, "CodeBuddyExtension", "Data", "Public", "auth", authID+".info")
	if err := os.MkdirAll(filepath.Dir(auth), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(auth, []byte(info), 0o600); err != nil {
		t.Fatal(err)
	}
	up := httptest.NewServer(st)
	t.Cleanup(up.Close)
	t.Cleanup(provider.WorkBuddyBaseForTest(up.URL, up.URL))
	for _, p := range provider.All() {
		if p.ID == id {
			return p
		}
	}
	t.Fatalf("%s isn't signed in", id)
	return provider.Provider{}
}

// A WorkBuddy plan's image models are those its config tags as drawing, not
// a list of magpie's; an edit goes to the model that edits beside the one
// named; the image comes back as WorkBuddy gave it (a URL) out of its
// envelope; AutoDrawer never picks one, as each image costs credits.
func TestWorkBuddyDrawsWithItsConfigsModels(t *testing.T) {
	const kept = "https://cos.example/img-1.png?sign=x"
	st := &wbStudio{models: wbHunyuanModels, answer: func(path string, body map[string]any) string {
		if path == "/v2/images/edits" {
			return `{"code":0,"msg":"OK","data":{"created":1,"data":[{"b64_json":"` + base64.StdEncoding.EncodeToString(pngBytes) + `"}],"usage":{"input_tokens":3,"output_tokens":7}}}`
		}
		return `{"code":0,"msg":"OK","requestId":"r1","data":{"created":1,"data":[{"url":"` + kept + `"}],"usage":{"input_tokens":333,"output_tokens":612,"credit":5.71}}}`
	}}
	p := wbSignedIn(t, "workbuddy", "workbuddy-desktop", st)

	// the first look asks the config in the background; the next has it
	Drawers(p)
	deadline := time.Now().Add(5 * time.Second)
	var ds []string
	for time.Now().Before(deadline) {
		ds = ds[:0]
		for _, m := range Drawers(p) {
			ds = append(ds, m.ID)
		}
		if len(ds) > 0 {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if strings.Join(ds, ",") != "hunyuan-image-alpha,hunyuan-image-v3.0-art" {
		t.Fatalf("drawers %v", ds)
	}
	for _, m := range Drawers(p) {
		if m.Images != (m.ID == "hunyuan-image-alpha") || m.Provider != "workbuddy" {
			t.Fatalf("drawer %+v", m)
		}
	}
	Drawers(p)
	if n := st.count("GET /v3/config"); n != 1 {
		t.Fatalf("the config was asked %d times", n)
	}
	if a := AutoDrawer(); a != "" {
		t.Fatalf("AutoDrawer picks %q", a)
	}

	s := New()
	code, _, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"model":"workbuddy/hunyuan-image-v3.0-art","prompt":"a magpie","size":"1024x1024","n":1,"quality":"high"}`)
	if code != 200 || !strings.Contains(raw, `"url":"`+kept+`"`) {
		t.Fatalf("%d %s", code, raw)
	}
	body, h := st.got("/v2/images/generations")
	if body["model"] != "hunyuan-image-v3.0-art" || body["size"] != "1024x1024" || body["n"] != 1.0 || body["prompt"] != "a magpie" {
		t.Fatalf("sent %v", body)
	}
	if _, ok := body["response_format"]; ok {
		t.Fatalf("Hunyuan was sent OpenAI's fields: %v", body)
	}
	if _, ok := body["quality"]; ok {
		t.Fatalf("Hunyuan was sent OpenAI's fields: %v", body)
	}
	if h.Get("Authorization") != "Bearer tok" || !strings.HasPrefix(h.Get("User-Agent"), "WorkBuddy/") || h.Get("X-User-Id") != "u1" {
		t.Fatalf("signed %v", h)
	}

	png := "data:image/png;base64," + base64.StdEncoding.EncodeToString(pngBytes)
	code, a, raw := postImages(t, s, "/v1/images/edits", "application/json", `{"model":"workbuddy/hunyuan-image-alpha","prompt":"make it blue","image":"`+png+`"}`)
	if code != 200 || len(a.Data) != 1 || a.Data[0].B64 == "" || a.Usage.Output != 7 {
		t.Fatalf("%d %s", code, raw)
	}
	body, _ = st.got("/v2/images/edits")
	imgs, _ := body["image"].([]any)
	if body["model"] != "hunyuan-image-alpha-edit" || len(imgs) != 1 || imgs[0] != png {
		t.Fatalf("edit sent %v", body)
	}

	// one with no model to edit with says so, and asks nothing
	code, _, raw = postImages(t, s, "/v1/images/edits", "application/json", `{"model":"workbuddy/hunyuan-image-v3.0-art","prompt":"make it blue","image":"`+png+`"}`)
	if code != 400 || !strings.Contains(raw, "no model to edit") {
		t.Fatalf("%d %s", code, raw)
	}
}

// WorkBuddy AI's GPT Image is asked as OpenAI's, b64_json and all, and edits
// itself; WorkBuddy's refusal in its envelope is an error with its message.
func TestWorkBuddyAIDrawsGPTImage(t *testing.T) {
	var refuse atomic.Bool
	st := &wbStudio{models: wbGPTImageModels, answer: func(path string, body map[string]any) string {
		if refuse.Load() {
			return `{"code":14018,"msg":"credits used up","data":null}`
		}
		return `{"code":0,"msg":"OK","data":{"created":1,"data":[{"b64_json":"` + base64.StdEncoding.EncodeToString(pngBytes) + `","revised_prompt":"a magpie, painted"}]}}`
	}}
	p := wbSignedIn(t, provider.WorkBuddyAIID, "workbuddy-desktop-ai", st)
	if _, err := wbImageModelsOf(context.Background(), wbConfigClient, p, true); err != nil {
		t.Fatal(err)
	}
	ds := Drawers(p)
	if len(ds) != 1 || ds[0].ID != "gpt-image-2.5-sunburst" || ds[0].Name != "GPT-Image-2.5-Sunburst" || !ds[0].Images {
		t.Fatalf("drawers %+v", ds)
	}
	s := New()
	code, a, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"model":"workbuddy-ai/gpt-image-2.5-sunburst","prompt":"a magpie","size":"1024x1024","quality":"low"}`)
	if code != 200 || len(a.Data) != 1 || a.Data[0].B64 == "" || !strings.Contains(raw, "a magpie, painted") {
		t.Fatalf("%d %s", code, raw)
	}
	body, _ := st.got("/v2/images/generations")
	if body["response_format"] != "b64_json" || body["quality"] != "low" || body["model"] != "gpt-image-2.5-sunburst" {
		t.Fatalf("sent %v", body)
	}
	png := "data:image/png;base64," + base64.StdEncoding.EncodeToString(pngBytes)
	code, _, raw = postImages(t, s, "/v1/images/edits", "application/json", `{"model":"workbuddy-ai/gpt-image-2.5-sunburst","prompt":"make it blue","image":["`+png+`"]}`)
	if code != 200 {
		t.Fatalf("%d %s", code, raw)
	}
	if body, _ = st.got("/v2/images/edits"); body["model"] != "gpt-image-2.5-sunburst" || body["response_format"] != "b64_json" {
		t.Fatalf("edit sent %v", body)
	}
	refuse.Store(true)
	code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"model":"workbuddy-ai/gpt-image-2.5-sunburst","prompt":"a magpie"}`)
	if code != 502 || !strings.Contains(raw, "credits used up (14018)") {
		t.Fatalf("%d %s", code, raw)
	}
}

// A WorkBuddy moved onto its plugin draws the same way: its config and its
// images are asked through the plugin's fetch, which signs them; the
// config, outside the plugin's /v2 base, at the plugin's own host.
func TestWorkBuddyPluginDraws(t *testing.T) {
	wbImages.Lock()
	wbImages.m = map[string]*wbImageList{}
	wbImages.Unlock()
	st := &wbStudio{models: wbHunyuanModels, answer: func(path string, body map[string]any) string {
		return `{"code":0,"msg":"OK","data":{"data":[{"b64_json":"` + base64.StdEncoding.EncodeToString(pngBytes) + `"}]}}`
	}}
	pid := besideFake(t, "workbuddy", st)
	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	if !drawsWorkBuddy(*p) {
		t.Fatal("the WorkBuddy plugin doesn't draw")
	}
	if _, err := wbImageModelsOf(context.Background(), wbConfigClient, *p, true); err != nil {
		t.Fatal(err)
	}
	if ds := Drawers(*p); len(ds) != 2 || ds[0].ID != "hunyuan-image-alpha" || ds[0].Provider != pid {
		t.Fatalf("drawers %+v", ds)
	}
	code, a, raw := postImages(t, New(), "/v1/images/generations", "application/json", `{"model":"`+pid+`/hunyuan-image-alpha","prompt":"a magpie"}`)
	if code != 200 || len(a.Data) != 1 {
		t.Fatalf("%d %s", code, raw)
	}
	if st.count("GET /v3/config") != 1 || st.count("POST /v1/images/generations") != 1 {
		st.mu.Lock()
		defer st.mu.Unlock()
		t.Fatalf("asked %v", st.asked)
	}
	if body, _ := st.got("/v1/images/generations"); body["model"] != "hunyuan-image-alpha" {
		t.Fatalf("sent %v", body)
	}
	// the plugin signs a URL of its own host only, never another's
	req, _ := http.NewRequest(http.MethodGet, "https://elsewhere.example/v3/config", nil)
	if res, err := p.Do(http.DefaultClient, req); err == nil || !strings.Contains(err.Error(), "not elsewhere.example") {
		if res != nil {
			res.Body.Close()
		}
		t.Fatalf("sent elsewhere: %v", err)
	}
}
