package gateway

import (
	"encoding/json"
	"net/http/httptest"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// Codex sends /fast's service_tier only on a model whose entry lists it
// (codex-rs ModelInfo::service_tier_for_request), so a routing group or a
// model with a fast mode that isn't a ChatGPT account's GPT is offered
// Fast in the list too, and one with no member that has a fast mode is
// offered none (cute_lynx_36404 on Discord: /fast did nothing through
// magpie, only the Routing page's default did).
func TestCodexFastOfferedWhereAMemberHasIt(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	for _, p := range []provider.Provider{
		{ID: "an", Name: "Anthropic", Key: "k", Anthropic: "https://api.anthropic.com", Models: []string{"claude-opus-5-5", "claude-sonnet-5"}},
		{ID: "zp", Name: "Zhipu", Key: "k", Chat: "http://zp.invalid/v1", Models: []string{"glm-5"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	for _, g := range []provider.Group{
		{ID: "hard", Name: "Hard", Members: []string{"zp/glm-5", "an/claude-opus-5-5"}, Routing: provider.Ordered},
		{ID: "cheap", Name: "Cheap", Members: []string{"zp/glm-5", "an/claude-sonnet-5"}, Routing: provider.Ordered},
	} {
		if err := provider.SaveGroup(g); err != nil {
			t.Fatal(err)
		}
	}
	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/v1/models", nil))
	var list struct {
		Data []struct {
			ID    string `json:"id"`
			Tiers []struct {
				ID string `json:"id"`
			} `json:"service_tiers"`
		} `json:"data"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatalf("%v: %s", err, rec.Body)
	}
	got := map[string]string{}
	for _, m := range list.Data {
		s := ""
		for _, x := range m.Tiers {
			s += x.ID + " "
		}
		got[m.ID] = s
	}
	for id, want := range map[string]string{
		"group/hard": "priority ", "group/cheap": "",
		"an/claude-opus-5-5": "priority ", "an/claude-sonnet-5": "", "zp/glm-5": "",
	} {
		if s, ok := got[id]; !ok || s != want {
			t.Errorf("%s tiers %q (listed %v), want %q; list %v", id, s, ok, want, got)
		}
	}
}
