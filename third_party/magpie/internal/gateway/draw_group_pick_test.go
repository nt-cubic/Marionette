package gateway

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// 7up6666 on X: a routing group of image models, picked in Settings ›
// Images, is what a request naming no model draws with, the Magpie Image
// MCP's included: on its members in its routing's order, the next asked
// while one fails. A group with no member that draws is no pick (it shows
// as missing, and Automatic draws in its place), nor is one whose members
// are all gone.
func TestImageGroupPickedInSettingsDraws(t *testing.T) {
	s, e := easeled(t)
	t.Setenv("MAGPIE_PUBLIC_URL", "")
	t.Cleanup(s.imageStore.running.Wait)
	var brokeAsked atomic.Int32
	broke := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		brokeAsked.Add(1)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(502)
		io.WriteString(w, `{"error":{"message":"upstream error: bad gateway","type":"upstream_error"}}`)
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
	for _, g := range []provider.Group{
		{ID: "painters", Name: "Painters", Members: []string{"flare-a/gpt-image-2.5-flare", "art/gpt-image-2.5-flare"}, Routing: "order"},
		{ID: "talkers", Name: "Talkers", Members: []string{"art/text"}},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	if !GroupDraws("group/painters") || GroupDraws("group/talkers") || GroupDraws("group/nope") {
		t.Fatal("GroupDraws")
	}
	if ListedDrawer("group/painters") != "group/painters" || DrawerHidden("group/painters") {
		t.Fatal("a group pick is listed as it is, and never hidden")
	}
	pick := func(v string) {
		t.Helper()
		st := settings.Load()
		st.ImageGen = v
		if err := settings.Save(st); err != nil {
			t.Fatal(err)
		}
	}

	pick("group/painters")
	if Drawer() != "group/painters" || DrawerMissing() != "" {
		t.Fatalf("the group pick isn't what draws: %q missing %q", Drawer(), DrawerMissing())
	}
	code, a, raw := postImages(t, s, "/v1/images/generations", "application/json", `{"prompt":"a magpie"}`)
	if code != 200 || len(a.Data) != 1 || a.Model != "art/gpt-image-2.5-flare" || brokeAsked.Load() == 0 {
		t.Fatalf("drawn with no model named: %d %s (first member asked %d)", code, raw, brokeAsked.Load())
	}
	// the MCP draws with the pick too
	h := lanGuard(s.Handler())
	before := len(e.got("/v1/images/generations"))
	m := readImageReply(t, imagePost(h, "127.0.0.1:5000", imageToolCall(GenerateImageTool, map[string]any{"prompt": "a magpie"})))
	if m.Result.IsError || len(e.got("/v1/images/generations")) != before+1 || !strings.Contains(m.Result.Content[0].Text, "art/gpt-image-2.5-flare") {
		t.Fatalf("MCP: %+v", m)
	}

	// a group that draws nothing is said missing, and Automatic draws
	pick("group/talkers")
	if DrawerMissing() != "group/talkers" || Drawer() == "group/talkers" || Drawer() == "" {
		t.Fatalf("chat-only group: drawer %q missing %q", Drawer(), DrawerMissing())
	}
	// as is one whose drawing members are all gone
	pick("group/painters")
	if err := provider.Delete("flare-a"); err != nil {
		t.Fatal(err)
	}
	art, _ = provider.Find("art")
	art.Models = []string{"text"}
	if err := provider.Save(*art); err != nil {
		t.Fatal(err)
	}
	if DrawerMissing() != "group/painters" || Drawer() == "group/painters" {
		t.Fatalf("group with no drawers left: drawer %q missing %q", Drawer(), DrawerMissing())
	}
}
