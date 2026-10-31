package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// The model requests masking would change go to instead (jessyoungyu on
// X) is set on its own: Settings › Privacy lists only models whose every
// route goes unmasked, on this computer or the network; one that would
// reach a vendor is refused with the reason; Off clears it; and the
// general save leaves it as it was.
func TestRedactRouteSetting(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	for _, p := range []provider.Provider{
		{ID: "cloud", Name: "Cloud", Key: "k", Models: []string{"big"}, Chat: "https://api.example.com/v1"},
		{ID: "near", Name: "Near", Key: "k", Models: []string{"qwen"}, Chat: "http://127.0.0.1:11434/v1", Unredacted: true},
		{ID: "leaky", Name: "Leaky", Key: "k", Models: []string{"m"}, Chat: "http://127.0.0.1:11435/v1", Unredacted: true, Fallback: []string{"cloud/big"}},
	} {
		if err := provider.Save(p); err != nil {
			t.Fatal(err)
		}
	}
	srv := Handler(nil, nil)
	do := func(method, path, body string) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		srv.ServeHTTP(rec, httptest.NewRequest(method, path, strings.NewReader(body)))
		return rec
	}
	var st struct {
		MaskRouteModels []struct{ ID string } `json:"maskRouteModels"`
		RedactRouteTo   string                `json:"redactRouteTo"`
	}
	read := func() {
		t.Helper()
		st.MaskRouteModels, st.RedactRouteTo = nil, ""
		rec := do("GET", "/api/settings", "")
		if err := json.Unmarshal(rec.Body.Bytes(), &st); err != nil {
			t.Fatalf("%v: %s", err, rec.Body)
		}
	}
	read()
	if len(st.MaskRouteModels) != 1 || st.MaskRouteModels[0].ID != "near/qwen" || st.RedactRouteTo != "" {
		t.Fatalf("listed %+v, set %q", st.MaskRouteModels, st.RedactRouteTo)
	}
	for _, id := range []string{"cloud/big", "leaky/m", "nowhere/x"} {
		if rec := do("POST", "/api/settings/redact-route", `{"model":"`+id+`"}`); rec.Code == http.StatusOK || !strings.Contains(rec.Body.String(), "unmasked") {
			t.Fatalf("%s taken: %d %s", id, rec.Code, rec.Body)
		}
	}
	if settings.Load().RedactRouteTo != "" {
		t.Fatal("a refused model was saved")
	}
	if rec := do("POST", "/api/settings/redact-route", `{"model":"near/qwen"}`); rec.Code != http.StatusOK || settings.Load().RedactRouteTo != "near/qwen" {
		t.Fatalf("near/qwen: %d %s", rec.Code, rec.Body)
	}
	// the general save doesn't carry it, and keeps it
	if rec := do("POST", "/api/settings", `{"redact":true}`); rec.Code != http.StatusOK || settings.Load().RedactRouteTo != "near/qwen" {
		t.Fatalf("general save: %d %s, now %q", rec.Code, rec.Body, settings.Load().RedactRouteTo)
	}
	read()
	if st.RedactRouteTo != "near/qwen" {
		t.Fatalf("read back %q", st.RedactRouteTo)
	}
	if rec := do("POST", "/api/settings/redact-route", `{"model":""}`); rec.Code != http.StatusOK || settings.Load().RedactRouteTo != "" {
		t.Fatalf("off: %d %s", rec.Code, rec.Body)
	}
	if err := settings.Save(settings.Settings{RedactRouteTo: "qwen"}); err == nil {
		t.Fatal("a bare name saved")
	}
}
