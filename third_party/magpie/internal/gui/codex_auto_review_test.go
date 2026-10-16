package gui

import (
	"encoding/json"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/agentenv"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// Settings' Codex auto-review model (#938) is set on its own: a model or
// group magpie serves (one it doesn't, or a name that is no model's id, is
// refused and the value kept), or back to Codex's own pick; the Settings
// page's other saves keep it; and Codex's list's tag changes with it, so
// Codex asks for the list again.
func TestCodexAutoReviewSetting(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	t.Setenv("APPDATA", filepath.Join(h, "AppData", "Roaming"))
	t.Setenv("LOCALAPPDATA", filepath.Join(h, "AppData", "Local"))
	for _, v := range agentenv.Vars {
		t.Setenv(v, "")
	}
	if err := provider.Save(provider.Provider{ID: "fake", Name: "Fake", Key: "k", Models: []string{"m1", "m2"}, Chat: "http://127.0.0.1:9/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "hidden", Name: "Hidden", Key: "k", Unlisted: true, Models: []string{"reviewer"}, Chat: "http://127.0.0.1:9/v1"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "checks", Name: "Checks", Members: []string{"fake/m2"}}); err != nil {
		t.Fatal(err)
	}
	call := func(path, body string) (int, map[string]any) {
		t.Helper()
		rec := httptest.NewRecorder()
		Handler(nil, nil).ServeHTTP(rec, httptest.NewRequest("POST", path, strings.NewReader(body)))
		var out map[string]any
		json.Unmarshal(rec.Body.Bytes(), &out)
		return rec.Code, out
	}

	tag := provider.CodexListTag()
	code, out := call("/api/settings/codex-auto-review", `{"model":"fake/m1"}`)
	if code != 200 || out["codexAutoReview"] != "fake/m1" || settings.Load().CodexAutoReview != "fake/m1" {
		t.Fatalf("a model: %d %v", code, out["codexAutoReview"])
	}
	if provider.CodexListTag() == tag {
		t.Error("Codex's list's tag didn't change with the setting")
	}
	if code, _ := call("/api/settings", `{"theme":"dark"}`); code != 200 || settings.Load().CodexAutoReview != "fake/m1" {
		t.Errorf("another save (%d) lost the setting: %q", code, settings.Load().CodexAutoReview)
	}
	// Groups and models of an unlisted provider are still valid targets.
	// Restore m1 before checking that rejected choices leave it in place.
	for _, id := range []string{"group/checks", "hidden/reviewer", "fake/m1"} {
		if code, _ := call("/api/settings/codex-auto-review", `{"model":"`+id+`"}`); code != 200 || settings.Load().CodexAutoReview != id {
			t.Fatalf("valid reviewer %s: %d, %q", id, code, settings.Load().CodexAutoReview)
		}
	}
	keptTag := provider.CodexListTag()
	for _, bad := range []string{"nope/x", "m1", "off", "fake/missing", "fake/", "group/missing"} {
		if code, _ := call("/api/settings/codex-auto-review", `{"model":"`+bad+`"}`); code < 400 || settings.Load().CodexAutoReview != "fake/m1" {
			t.Errorf("%s: %d, %q", bad, code, settings.Load().CodexAutoReview)
		}
		if provider.CodexListTag() != keptTag {
			t.Errorf("rejecting %s changed Codex's catalog tag", bad)
		}
	}
	if code, out := call("/api/settings/codex-auto-review", `{"model":" fake/m2 "}`); code != 200 || out["codexAutoReview"] != "fake/m2" {
		t.Errorf("another model: %d %v", code, out["codexAutoReview"])
	}
	if code, _ := call("/api/settings/codex-auto-review", `{"model":""}`); code != 200 || settings.Load().CodexAutoReview != "" {
		t.Errorf("Codex's own: %d %q", code, settings.Load().CodexAutoReview)
	}
	if provider.CodexListTag() != tag {
		t.Error("Codex's list's tag isn't back with the setting unset")
	}
}
