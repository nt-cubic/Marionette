package gateway

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"reflect"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A plugin's model is listed with every kind of input and output the
// plugin says it has, not image alone, and with its levels; one that
// thinks with no level to pick says it thinks and offers none (#1595,
// tkhs101). The two models are opencode-zen-free 0.1.3's, as its config
// hook writes muse-spark and its models hook lists mimo, from
// models.opencode.ai's own entries. Whether a model takes images is still
// the user's to say.
func TestPluginModelModalities(t *testing.T) {
	t.Setenv("FAKE_ZEN", "1")
	pid := besideFake(t, "opencode-zen-free", &cursorUp{})

	type listed struct {
		Reasoning  bool                      `json:"reasoning"`
		Levels     []struct{ Effort string } `json:"supported_reasoning_levels"`
		Modalities *struct {
			Input  []string `json:"input"`
			Output []string `json:"output"`
		} `json:"modalities"`
	}
	models := func() map[string]listed {
		t.Helper()
		rec := httptest.NewRecorder()
		New().Handler().ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/v1/models", nil))
		var list struct {
			Data []struct {
				ID string `json:"id"`
				listed
			} `json:"data"`
		}
		if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
			t.Fatalf("/v1/models: %v\n%s", err, rec.Body.String())
		}
		out := map[string]listed{}
		for _, m := range list.Data {
			out[m.ID] = m.listed
		}
		return out
	}
	efforts := func(m listed) []string {
		out := []string{}
		for _, l := range m.Levels {
			out = append(out, l.Effort)
		}
		return out
	}
	check := func(got map[string]listed, id string, in, out, levels []string) {
		t.Helper()
		m, ok := got[pid+"/"+id]
		if !ok {
			t.Fatalf("%s not listed: %v", id, got)
		}
		if m.Modalities == nil {
			t.Fatalf("%s: no modalities", id)
		}
		if !reflect.DeepEqual(m.Modalities.Input, in) || !reflect.DeepEqual(m.Modalities.Output, out) {
			t.Errorf("%s: modalities in %v out %v, want in %v out %v", id, m.Modalities.Input, m.Modalities.Output, in, out)
		}
		if !m.Reasoning || !reflect.DeepEqual(efforts(m), levels) {
			t.Errorf("%s: reasoning %v at %v, want true at %v", id, m.Reasoning, efforts(m), levels)
		}
	}

	got := models()
	check(got, "muse-spark-1.3-contributor-free", []string{"text", "image", "audio", "video", "pdf"}, []string{"text"}, []string{"minimal", "low", "medium", "high", "xhigh"})
	check(got, "mimo-v2.6-flash-free", []string{"text", "image", "audio", "video"}, []string{"text"}, []string{})
	// a model the plugin says nothing of takes what isn't known
	if m := got[pid+"/fake-1"]; m.Modalities != nil && m.Modalities.Input != nil {
		t.Errorf("fake-1, not said of, takes %v", m.Modalities.Input)
	}

	// the user says mimo takes no images: the rest it takes stays
	no := false
	if err := provider.SetModelImage(pid+"/mimo-v2.6-flash-free", &no); err != nil {
		t.Fatal(err)
	}
	check(models(), "mimo-v2.6-flash-free", []string{"text", "audio", "video"}, []string{"text"}, []string{})
}
