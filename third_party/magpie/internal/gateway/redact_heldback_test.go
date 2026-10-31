package gateway

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// A turn that masks nothing still gets its placeholders back: the vendor
// can answer with one it was sent turns ago and holds on its side (Codex's
// compacted history, a server-side conversation), while the request in
// hand carries no value to mask. yapechan123 saw a table of
// {{PERSON_…}}、{{PERSON_…}} come back unrestored.
func TestReplyToAnUnmaskedTurnIsRestored(t *testing.T) {
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	const email = "jeremy.zhou@gmail.com"
	placeholder := regexp.MustCompile(`\{\{[A-Z_]+_[a-z2-7]{8}\}\}`)
	var held string
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		if p := placeholder.FindString(string(b)); p != "" {
			held = p // the vendor keeps it
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]any{
			"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-sonnet-4-5",
			"content":     []any{map[string]any{"type": "text", "text": "| 负责人 | " + held + "、" + held + " |"}},
			"stop_reason": "end_turn", "usage": map[string]any{"input_tokens": 10, "output_tokens": 5},
		})
	}))
	defer up.Close()
	if err := provider.Save(provider.Provider{ID: "zen", Name: "Zen", Key: "k", Models: []string{"claude-sonnet-4-5"}, Anthropic: up.URL}); err != nil {
		t.Fatal(err)
	}
	if err := settings.Save(settings.Settings{Redact: true, RedactPersonal: true}); err != nil {
		t.Fatal(err)
	}
	code, body := post(t, "/v1/messages", `{"model":"zen/claude-sonnet-4-5","max_tokens":100,"messages":[{"role":"user","content":"I am `+email+`"}]}`)
	if code != 200 || held == "" {
		t.Fatalf("first turn: %d %s (held %q)", code, body, held)
	}
	code, body = post(t, "/v1/messages", `{"model":"zen/claude-sonnet-4-5","max_tokens":100,"messages":[{"role":"user","content":"write the table"}]}`)
	if code != 200 {
		t.Fatalf("second turn: %d %s", code, body)
	}
	if strings.Contains(body, "{{") || !strings.Contains(body, email+"、"+email) {
		t.Fatalf("the reply to a turn that masked nothing kept its placeholder: %s", body)
	}
}
