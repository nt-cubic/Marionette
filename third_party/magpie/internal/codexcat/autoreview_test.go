package codexcat

import (
	"encoding/json"
	"testing"

	"github.com/yetone/magpie/internal/catalog"
	"github.com/yetone/magpie/internal/settings"
)

func overrides(t *testing.T, ms []catalog.Model) map[string]any {
	t.Helper()
	var got struct {
		Models []map[string]any `json:"models"`
	}
	if err := json.Unmarshal(Catalog(ms), &got); err != nil {
		t.Fatal(err)
	}
	out := map[string]any{}
	for _, m := range got.Models {
		v, ok := m["auto_review_model_override"]
		if !ok {
			v = "none"
		}
		out[m["slug"].(string)] = v
	}
	return out
}

// With settings.CodexAutoReview every entry magpie hands Codex — the ChatGPT
// account's own and magpie's — names that model as
// auto_review_model_override, which Codex's auto-review runs on (#938); with
// none set no entry has the field, and one magpie left in Codex's cache is
// taken out.
func TestCodexAutoReviewEntries(t *testing.T) {
	v1Home(t, `{"etag":"W/\"a\"","models":[{"slug":"gpt-6","base_instructions":"x"},{"slug":"gpt-7","base_instructions":"x","auto_review_model_override":"deepseek/old"},{"slug":"gpt-8","base_instructions":"x","auto_review_model_override":"codex-auto-review"}]}`)
	ms := []catalog.Model{
		{ID: "codex/gpt-6", Name: "GPT-6 · Codex"},
		{ID: "codex/gpt-7", Name: "GPT-7 · Codex"},
		{ID: "codex/gpt-8", Name: "GPT-8 · Codex"},
		{ID: "codex/gpt-9", Name: "GPT-9 · Codex"},
		{ID: "group/smart", Name: "smart"},
		{ID: "anthropic/claude-opus", Name: "Opus"},
	}
	off := overrides(t, ms)
	want := map[string]any{"codex/gpt-6": "none", "codex/gpt-7": "none", "codex/gpt-8": "codex-auto-review",
		"codex/gpt-9": "none", "group/smart": "none", "anthropic/claude-opus": "none"}
	for k, v := range want {
		if off[k] != v {
			t.Fatalf("unset: %s = %v, want %v (%v)", k, off[k], v, off)
		}
	}

	s := settings.Load()
	s.CodexAutoReview = "deepseek/deepseek-flash"
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	on := overrides(t, ms)
	for k := range want {
		if on[k] != "deepseek/deepseek-flash" {
			t.Fatalf("set: %s = %v (%v)", k, on[k], on)
		}
	}
}

// The setting must be a model's id.
func TestCodexAutoReviewChecked(t *testing.T) {
	v1Home(t, `{"models":[]}`)
	s := settings.Load()
	s.CodexAutoReview = "flash"
	if err := settings.Save(s); err == nil {
		t.Fatal("a value that is no model's id was saved")
	}
	s.CodexAutoReview = "  group/smart "
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if got := settings.Load().CodexAutoReview; got != "group/smart" {
		t.Fatalf("saved %q", got)
	}
}
