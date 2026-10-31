package gateway

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/yetone/magpie/internal/access"
)

// mcpModelProp is the model argument tools/list gives tool, for the key
// in hdr.
func mcpModelProp(t *testing.T, h http.Handler, addr, tool string, hdr ...string) (enum []string, desc string) {
	t.Helper()
	var list struct {
		Result struct {
			Tools []struct {
				Name        string
				InputSchema struct {
					Properties map[string]struct {
						Enum        []string
						Description string
					}
				}
			}
		}
	}
	w := imagePost(h, addr, `{"jsonrpc":"2.0","id":2,"method":"tools/list"}`, hdr...)
	json.Unmarshal(w.Body.Bytes(), &list)
	for _, tl := range list.Result.Tools {
		if tl.Name == tool {
			p, ok := tl.InputSchema.Properties["model"]
			if !ok {
				t.Fatalf("%s takes no model: %s", tool, w.Body.String())
			}
			return p.Enum, p.Description
		}
	}
	t.Fatalf("no %s: %s", tool, w.Body.String())
	return nil, ""
}

// generate_image takes the model to draw with (XYenon on Discord): one the
// schema lists, by its whole id or a bare one, is the one drawn with; left
// out, Settings' is; one it doesn't list is an error naming those it
// does, and nothing is drawn.
func TestImageMCPDrawsWithTheModelAsked(t *testing.T) {
	s, e := easeled(t)
	t.Cleanup(s.imageStore.running.Wait)
	h := lanGuard(s.Handler())
	const here = "127.0.0.1:5000"
	enum, desc := mcpModelProp(t, h, here, GenerateImageTool)
	if !slices.Contains(enum, "art/gpt-image-1") || !slices.Contains(enum, "art/painter-image-preview") || slices.Contains(enum, "art/text") || enum[0] != Drawer() {
		t.Fatalf("enum %v (default %s)", enum, Drawer())
	}
	if !strings.Contains(desc, "now "+Drawer()) {
		t.Fatalf("description %q", desc)
	}
	// left out: the default, at the images API
	m := readImageReply(t, imagePost(h, here, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie"})))
	if m.Result.IsError || len(e.got("/v1/images/generations")) != 1 || len(e.got("/v1/chat/completions")) != 0 {
		t.Fatalf("default: %+v %v", m, e.got("/v1/chat/completions"))
	}
	// asked by its whole id, then by its bare one: drawn with it
	for i, model := range []string{"art/painter-image-preview", "painter-image-preview"} {
		m = readImageReply(t, imagePost(h, here, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie", "model": model})))
		if sent := e.got("/v1/chat/completions"); m.Result.IsError || len(sent) != i+1 || !strings.Contains(sent[i], "painter-image-preview") || !strings.Contains(m.Result.Content[0].Text, "art/painter-image-preview") {
			t.Fatalf("%s: %+v, sent %v", model, m, sent)
		}
	}
	if n := len(e.got("/v1/images/generations")); n != 1 {
		t.Fatalf("the default drew %d times", n)
	}
	// unknown, or not an image model: an error naming those there are
	for _, model := range []string{"art/nope", "art/text", "nope"} {
		m = readImageReply(t, imagePost(h, here, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie", "model": model})))
		if txt := m.Result.Content[0].Text; !m.Result.IsError || !strings.Contains(txt, `"`+model+`"`) || !strings.Contains(txt, "art/gpt-image-1") || !strings.Contains(txt, "art/painter-image-preview") {
			t.Fatalf("%s: %+v", model, m)
		}
	}
	if len(e.got("/v1/images/generations")) != 1 || len(e.got("/v1/chat/completions")) != 2 {
		t.Fatalf("an unknown model drew: %v %v", e.got("/v1/images/generations"), e.got("/v1/chat/completions"))
	}
	// the stdio server asks the same
	ask := func(model string) mediaReply {
		b, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": 9, "method": MediaModelMethod, "params": map[string]any{"tool": GenerateImageTool, "model": model}})
		return readMCPRaw(t, imagePost(h, here, string(b)))
	}
	if r := ask("painter-image-preview"); r.Error != nil || !strings.Contains(string(r.Result), `"art/painter-image-preview"`) {
		t.Fatalf("media_model: %+v", r)
	}
	if r := ask("art/nope"); r.Error == nil || !strings.Contains(r.Error.Message, "art/gpt-image-1") {
		t.Fatalf("media_model, unknown: %+v", r)
	}
}

type mediaReply struct {
	Result json.RawMessage
	Error  *struct {
		Code    int
		Message string
	}
}

func readMCPRaw(t *testing.T, w *httptest.ResponseRecorder) mediaReply {
	t.Helper()
	var r mediaReply
	res := w.Result()
	defer res.Body.Close()
	if err := json.NewDecoder(res.Body).Decode(&r); err != nil {
		t.Fatal(err)
	}
	return r
}

// A gateway key held to some models is held to them here too: the schema
// lists only those, one it may not use is refused by the images API's own
// check, and nothing is drawn.
func TestImageMCPModelHeldToTheKey(t *testing.T) {
	s, e := easeled(t)
	t.Cleanup(s.imageStore.running.Wait)
	keys, secrets := newCaller(t, "Narrow")
	if _, err := access.Update("models-key", access.Change{Key: keys[0].ID, Models: []string{Drawer()}}); err != nil {
		t.Fatal(err)
	}
	h := lanGuard(s.Handler())
	const there = "192.0.2.7:4000"
	auth := []string{"Authorization", "Bearer " + secrets[0]}
	if enum, _ := mcpModelProp(t, h, there, GenerateImageTool, auth...); len(enum) != 1 || enum[0] != Drawer() {
		t.Fatalf("the key's enum %v", enum)
	}
	m := readImageReply(t, imagePost(h, there, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie", "model": "art/painter-image-preview"}), auth...))
	if txt := m.Result.Content[0].Text; !m.Result.IsError || !strings.Contains(txt, "may not use") || !strings.Contains(txt, `"Narrow"`) {
		t.Fatalf("a model the key may not use: %+v", m)
	}
	m = readImageReply(t, imagePost(h, there, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie", "model": "art/nope"}), auth...))
	if txt := m.Result.Content[0].Text; !m.Result.IsError || !strings.Contains(txt, Drawer()) || strings.Contains(txt, "painter") {
		t.Fatalf("unknown, for the key: %+v", m)
	}
	if n := len(e.got("/v1/chat/completions")) + len(e.got("/v1/images/generations")); n != 0 {
		t.Fatalf("drew %d times", n)
	}
	// the one it may use draws
	m = readImageReply(t, imagePost(h, there, imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie", "model": Drawer()}), auth...))
	if m.Result.IsError || len(e.got("/v1/images/generations")) != 1 {
		t.Fatalf("the key's model: %+v", m)
	}
}

// generate_video takes the model too: one listed reaches the vendor, one
// that isn't is an error naming those that are.
func TestImageMCPVideoModelAsked(t *testing.T) {
	grokSignedIn(t)
	g := newGrokMedia(t)
	s := New()
	h := lanGuard(s.Handler())
	was, wasPoll := imageMCPWithin, imageMCPPoll
	t.Cleanup(func() { imageMCPWithin, imageMCPPoll = was, wasPoll })
	imageMCPWithin, imageMCPPoll = time.Minute, time.Millisecond
	const here = "127.0.0.1:5000"
	enum, _ := mcpModelProp(t, h, here, GenerateVideoTool)
	if len(enum) == 0 || !strings.HasPrefix(enum[0], "grok/") {
		t.Fatalf("video enum %v", enum)
	}
	m := readImageReply(t, imagePost(h, here, imageToolCall(GenerateVideoTool, map[string]any{"prompt": "a kite", "model": "grok/nope-video"})))
	if txt := m.Result.Content[0].Text; !m.Result.IsError || !strings.Contains(txt, enum[0]) || g.body("/v1/videos/generations", 0) != "" {
		t.Fatalf("unknown video model: %+v", m)
	}
	m = readImageReply(t, imagePost(h, here, imageToolCall(GenerateVideoTool, map[string]any{"prompt": "a kite", "model": enum[0]})))
	_, id, _ := strings.Cut(enum[0], "/")
	if sent := g.body("/v1/videos/generations", 0); m.Result.IsError || !strings.Contains(sent, `"model":"`+id+`"`) {
		t.Fatalf("video %s: %+v, sent %s", enum[0], m, sent)
	}
	// a newer model of the family Grok serves, before magpie lists it
	m = readImageReply(t, imagePost(h, here, imageToolCall(GenerateVideoTool, map[string]any{"prompt": "a kite", "model": "grok/grok-imagine-video-9"})))
	if sent := g.body("/v1/videos/generations", 1); m.Result.IsError || !strings.Contains(sent, `"model":"grok-imagine-video-9"`) {
		t.Fatalf("a newer video model: %+v, sent %s", m, sent)
	}
}
