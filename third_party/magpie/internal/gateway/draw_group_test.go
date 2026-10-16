package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #2005 (tianxia3111): an image request to a routing group set to "order"
// whose first member's relay answers 502 is drawn by the next member, as a
// chat to the group would be, for generations and edits alike; the member
// that failed is named in the call. A refusal of the request itself (a 400
// with no words of quota or an unserved model) is the request's, and is
// answered as it came, without asking the next member.
func TestImageGroupMovesOnToTheNextMember(t *testing.T) {
	s, e := easeled(t)
	var mu sync.Mutex
	var asked []string
	status, said := 502, `{"error":{"message":"upstream error: bad gateway","type":"upstream_error"}}`
	broke := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		asked = append(asked, r.URL.Path)
		code, body := status, said
		mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(code)
		io.WriteString(w, body)
	}))
	t.Cleanup(broke.Close)
	if err := provider.Save(provider.Provider{ID: "flare-a", Name: "Flare A", Chat: broke.URL + "/v1", Key: "ka", Models: []string{"gpt-image-2.5-flare"}}); err != nil {
		t.Fatal(err)
	}
	art, _ := provider.Find("art")
	art.Models = append(art.Models, "gpt-image-2.5-flare")
	if err := provider.Save(*art); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "gpt-image-2.5-flare", Name: "gpt-image-2.5-flare", Members: []string{"flare-a/gpt-image-2.5-flare", "art/gpt-image-2.5-flare"}, Routing: "order"}); err != nil {
		t.Fatal(err)
	}

	code, a, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"model":"group/gpt-image-2.5-flare","prompt":"a magpie","size":"1024x1024"}`)
	if code != 200 || len(a.Data) != 1 || a.Model != "art/gpt-image-2.5-flare" {
		t.Fatalf("the group's second member didn't draw: %d %s", code, raw)
	}
	if sent := e.got("/v1/images/generations"); len(sent) != 1 || !strings.Contains(sent[0], `"model":"gpt-image-2.5-flare"`) {
		t.Fatalf("the second member was sent %v", sent)
	}
	mu.Lock()
	first := len(asked)
	mu.Unlock()
	if first == 0 {
		t.Fatal("the group's first member was never asked")
	}
	if c := s.Recent()[0]; c.Provider != "art" || c.Status != 200 || !strings.Contains(c.Fallback, "502") {
		t.Fatalf("call recorded as %+v", c)
	}

	// an edit to the group goes the same way
	code, a, raw = postImages(t, s, "/v1/images/edits", "application/json", `{"model":"group/gpt-image-2.5-flare","prompt":"bluer","images":[{"image_url":"data:image/png;base64,iVBORw0KGgo="}]}`)
	if code != 200 || len(a.Data) != 1 || a.Model != "art/gpt-image-2.5-flare" {
		t.Fatalf("the group's second member didn't edit: %d %s", code, raw)
	}

	// a refusal of the prompt itself would be the same on every member
	mu.Lock()
	status, said = 400, `{"error":{"message":"Your request was rejected by the safety system.","type":"invalid_request_error"}}`
	mu.Unlock()
	before := len(e.got("/v1/images/generations"))
	code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"model":"group/gpt-image-2.5-flare","prompt":"a magpie"}`)
	if code != 400 || !strings.Contains(raw, "safety system") {
		t.Fatalf("the request's own refusal: %d %s", code, raw)
	}
	if n := len(e.got("/v1/images/generations")); n != before {
		t.Fatalf("a refusal of the request was sent on to the next member (%d asks)", n-before)
	}

	// every member failing answers with the last one's failure
	if err := provider.SaveGroup(provider.Group{ID: "dead-draw", Name: "dead-draw", Members: []string{"flare-a/gpt-image-2.5-flare"}, Routing: "order"}); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	status, said = 503, `{"error":{"message":"no available channel"}}`
	mu.Unlock()
	if code, _, raw = postImages(t, s, "/v1/images/generations", "application/json", `{"model":"group/dead-draw","prompt":"a magpie"}`); code != 503 || !strings.Contains(raw, "no available channel") {
		t.Fatalf("all failing: %d %s", code, raw)
	}
}

// #2005's sibling: a video started on a routing group goes on to the next
// member when the first's vendor answers 502, as an image does, and its id
// names the member that started it, which it is then asked after on.
func TestVideoGroupMovesOnToTheNextMember(t *testing.T) {
	fresh(t)
	var mu sync.Mutex
	var started int
	broke := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.Method == "GET" && r.URL.Path == "/v1/models" {
			io.WriteString(w, `{"object":"list","data":[{"id":"vid-1","object":"model","kind":"video"}]}`)
			return
		}
		mu.Lock()
		started++
		mu.Unlock()
		w.WriteHeader(502)
		io.WriteString(w, `{"error":{"message":"upstream error: bad gateway"}}`)
	}))
	t.Cleanup(broke.Close)
	saveFetched(t, provider.Provider{ID: "reel-a", Name: "Reel A", Chat: broke.URL + "/v1", Key: "sk-a"})
	up := newOpenAIVideos(t)
	saveFetched(t, provider.Provider{ID: "studio", Name: "Studio", Chat: up.URL + "/v1", Key: "sk-studio"})
	if err := provider.SaveGroup(provider.Group{ID: "reels", Name: "reels", Members: []string{"reel-a/vid-1", "studio/vid-1"}, Routing: "order"}); err != nil {
		t.Fatal(err)
	}
	h := New().Handler()
	code, obj, raw := asKey(t, h, "", "POST", "/v1/videos", `{"model":"group/reels","prompt":"a cat on a fence","seconds":4}`)
	id, _ := obj["id"].(string)
	if code != 200 || !strings.HasPrefix(id, "video_studio.") || obj["model"] != "studio/vid-1" {
		t.Fatalf("the group's second member didn't start it: %d %s", code, raw)
	}
	mu.Lock()
	n := started
	mu.Unlock()
	if n != 1 {
		t.Fatalf("the first member was asked %d times", n)
	}
	if code, obj, raw = asKey(t, h, "", "GET", "/v1/videos/"+id, ""); code != 200 || obj["status"] != "in_progress" {
		t.Fatalf("poll: %d %s", code, raw)
	}
}
