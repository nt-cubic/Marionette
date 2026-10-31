package gateway

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// A plugin model that says it gives no text is no model to talk with: its
// embedding and its TTS leave the chat lists, the gateway's /v1/models,
// the agents' (provider.Catalog) and the provider's own (Available, the
// GUI's chips), as XYenon's Google AI Studio plugin lists them (#3279). A
// model naming text among what it gives stays, speaking or drawing too.
// An embedding is known by what its plugin says, not its id (bge-m3).
func TestPluginModelGivingNoTextLeavesTheChatLists(t *testing.T) {
	t.Setenv("FAKE_KINDS", "1")
	pid := besideFake(t, "google-ai-studio", &cursorUp{})

	gone := []string{"gemini-embedding-001", "bge-m3", "gemini-2.5-flash-preview-tts"}
	kept := []string{"fake-1", "gemini-2.5-flash-native-audio", "gemini-2.5-flash-image"}
	check := func(where string, got map[string]bool) {
		t.Helper()
		for _, id := range gone {
			if got[id] {
				t.Errorf("%s lists %s, which gives no text", where, id)
			}
		}
		for _, id := range kept {
			if !got[id] {
				t.Errorf("%s lost %s", where, id)
			}
		}
	}

	rec := httptest.NewRecorder()
	New().Handler().ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/v1/models", nil))
	var list struct {
		Data []struct{ ID string } `json:"data"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &list); err != nil {
		t.Fatalf("/v1/models: %v\n%s", err, rec.Body.String())
	}
	got := map[string]bool{}
	for _, m := range list.Data {
		if id, ok := strings.CutPrefix(m.ID, pid+"/"); ok {
			got[id] = true
		}
	}
	check("/v1/models", got)

	got = map[string]bool{}
	for _, e := range provider.Catalog() {
		if e.Provider.ID == pid {
			got[e.Model] = true
		}
	}
	check("the agents' catalog", got)

	p, err := provider.Find(pid)
	if err != nil {
		t.Fatal(err)
	}
	got = map[string]bool{}
	for _, m := range p.Available() {
		got[m.ID] = true
	}
	check("Available", got)
}
