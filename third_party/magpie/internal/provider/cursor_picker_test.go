package provider

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
)

// The picker's models `cursor-agent models` leaves out are listed (ARNO on
// Discord: "cursor provider 里少了 glm-5.3 和 glm-5.3-flash"), against a
// fake Cursor; hidden, Tab-only, chat-only ones and a model the list already
// has (any of its slugs) aren't.
func TestCursorPickerModels(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/aiserver.v1.AiService/AvailableModels" || r.Header.Get("Authorization") != "Bearer tok" {
			http.Error(w, "no", http.StatusNotFound)
			return
		}
		var body map[string]any
		b, _ := io.ReadAll(r.Body)
		if json.Unmarshal(b, &body) != nil || body["useModelParameters"] != true {
			t.Errorf("body %s", b)
		}
		w.Header().Set("Content-Type", "application/json")
		io.WriteString(w, `{"models":[
			{"name":"gpt-5.6-luna","clientDisplayName":"GPT-5.6 Luna","supportsNonMaxMode":false,"variants":[
				{"displayName":"Low","isMaxMode":true,"legacySlug":"gpt-5.6-luna-low"},
				{"displayName":"High","isMaxMode":true,"legacySlug":"gpt-5.6-luna-high"}]},
			{"name":"composer-2.5","clientDisplayName":"Composer 2.5","variants":[{"legacySlug":"composer-2.5"}]},
			{"name":"glm-5.3","clientDisplayName":"GLM-5.3","contextTokenLimit":200000,"variants":[{"displayName":""}]},
			{"name":"glm-5.3-flash","clientDisplayName":"GLM-5.3 Flash"},
			{"name":"secret","clientDisplayName":"Secret","isHidden":true},
			{"name":"tab","onlySupportsCmdK":true},
			{"name":"chat","isChatOnly":true},
			{"name":"noagent","supportsAgent":false},
			{"name":"claude-4.5-haiku","clientDisplayName":"Haiku 4.5"}]}`)
	}))
	defer srv.Close()
	old := cursorBase
	cursorBase = srv.URL
	defer func() { cursorBase = old }()

	have := []catalog.Model{{ID: "gpt-5.6-luna-low"}, {ID: "composer-2.5"}}
	got := cursorPickerModels(context.Background(), "tok", have)
	want := []catalog.Model{
		{ID: "glm-5.3", Name: "GLM-5.3", Context: 200000},
		{ID: "glm-5.3-flash", Name: "GLM-5.3 Flash", Context: cursorContext("glm-5.3-flash", "GLM-5.3 Flash")},
	}
	if len(got) != len(want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
	for i := range want {
		if got[i].ID != want[i].ID || got[i].Name != want[i].Name || got[i].Context != want[i].Context {
			t.Errorf("model %d: got %+v, want %+v", i, got[i], want[i])
		}
	}

	// a picker Cursor can't give adds nothing
	cursorBase = srv.URL + "/down"
	if got := cursorPickerModels(context.Background(), "tok", have); len(got) != 0 {
		t.Errorf("picker down: got %+v", got)
	}
}
