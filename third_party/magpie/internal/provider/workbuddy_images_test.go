package provider

import (
	"context"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

// WorkBuddy AI's GLM models take no images, though its config says every
// model does: on 2026-10-13 a real account's image sent to glm-5.3-flash,
// glm-5.3 and glm-5.2 came back as "[System reminder: the current model
// does not support images. Content filtered. ...]" (Yirasumi on Discord).
// Its other models read them, and WorkBuddy (China)'s GLM does too, so
// only WorkBuddy AI's GLM models are listed text-only, and magpie's
// image-describing model reads their images.
func TestWorkBuddyAIGLMIsTextOnly(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(`{"code":0,"data":{"agents":[{"name":"cli","models":["glm-5.3-flash","glm-5.3","glm-5.2","glm-6","deepseek-v4.1-flash","kimi-k3","my-glm-5"]}],
			"models":[{"id":"glm-5.3-flash","supportsImages":true},{"id":"glm-5.3","supportsImages":true},{"id":"glm-5.2","supportsImages":true},
			 {"id":"glm-6","supportsImages":true},{"id":"deepseek-v4.1-flash","supportsImages":true},{"id":"kimi-k3","supportsImages":true},
			 {"id":"my-glm-5","supportsImages":true}]}}`))
	}))
	defer srv.Close()
	defer WorkBuddyBaseForTest(srv.URL, srv.URL)()
	sign := func(context.Context, *http.Request, []byte) error { return nil }
	images := func(w *wbSite) map[string]bool {
		ms, err := wbFetchModels(context.Background(), w, sign)
		if err != nil {
			t.Fatal(err)
		}
		out := map[string]bool{}
		for _, m := range ms {
			if m.ImageInput == nil || *m.ImageInput != m.Images {
				t.Errorf("%s %s: Images %v, ImageInput %v", w.id, m.ID, m.Images, m.ImageInput)
			}
			out[m.ID] = m.Images
		}
		return out
	}
	ai := images(wbAI)
	for id, want := range map[string]bool{"glm-5.3-flash": false, "glm-5.3": false, "glm-5.2": false, "glm-6": false,
		"deepseek-v4.1-flash": true, "kimi-k3": true, "my-glm-5": true} {
		if ai[id] != want {
			t.Errorf("WorkBuddy AI %s: images %v, want %v", id, ai[id], want)
		}
	}
	for id, v := range images(wbCN) {
		if !v {
			t.Errorf("WorkBuddy (China) %s lost its images", id)
		}
	}

	// the list shown before the config is read says the same, and the
	// site's own list is left as it was
	for _, m := range wbAI.textOnly(wbAI.models) {
		if m.ID == "glm-5.3" && (m.Images || m.ImageInput == nil || *m.ImageInput) {
			t.Errorf("WorkBuddy AI's built-in glm-5.3: %+v", m)
		}
	}
	for _, m := range wbAI.models {
		if m.ImageInput != nil {
			t.Errorf("wbAIModels %s changed: %+v", m.ID, m)
		}
	}

	// a user who said the model takes images keeps their answer
	ms, _ := wbFetchModels(context.Background(), wbAI, sign)
	for _, m := range ms {
		if m.ID != "glm-5.3-flash" {
			continue
		}
		p := Provider{ID: WorkBuddyAIID}
		if e := entryFor(p, m, settings.Settings{}); e.Images {
			t.Errorf("glm-5.3-flash with no answer of the user's: Images %v", e.Images)
		}
		s := settings.Settings{ModelImages: map[string]bool{WorkBuddyAIID + "/glm-5.3-flash": true}}
		if e := entryFor(p, m, s); !e.Images || e.ImageInput == nil || !*e.ImageInput {
			t.Errorf("the user's own yes was overridden: %+v", e)
		}
	}
}

// A list saved before this was known, with WorkBuddy AI's GLM taking
// images, reads text-only too, without waiting for the next fetch.
func TestWorkBuddyAISavedGLMIsTextOnly(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(home, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(home, ".cache"))
	yes := true
	saved := []catalog.Model{{ID: "glm-5.3-flash", Images: true, ImageInput: &yes}, {ID: "deepseek-v4.1-flash", Images: true, ImageInput: &yes}}
	for _, id := range []string{WorkBuddyAIID, "workbuddy"} {
		if err := catalog.SaveLive(id, "", saved); err != nil {
			t.Fatal(err)
		}
	}
	want := map[string]map[string]bool{
		WorkBuddyAIID: {"glm-5.3-flash": false, "deepseek-v4.1-flash": true},
		"workbuddy":   {"glm-5.3-flash": true, "deepseek-v4.1-flash": true},
	}
	for pid, w := range want {
		seen := 0
		for _, m := range (Provider{ID: pid}).Available() {
			v, ok := w[m.ID]
			if !ok {
				continue
			}
			seen++
			if m.ImageInput == nil || *m.ImageInput != v || m.Images != v {
				t.Errorf("%s %s: Images %v, ImageInput %v, want %v", pid, m.ID, m.Images, m.ImageInput, v)
			}
		}
		if seen != len(w) {
			t.Errorf("%s: %d of the saved models listed", pid, seen)
		}
	}
}
