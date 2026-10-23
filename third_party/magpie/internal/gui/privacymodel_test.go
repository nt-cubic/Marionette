package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/pii"
	"github.com/yetone/magpie/internal/settings"
)

// The local masking model is on only once it is on disk: turned on where
// it can't be downloaded, nothing is saved; the Settings page's own save
// never turns it on or off; and off is saved at once.
func TestPrivacyModelOnlyOnOnceDownloaded(t *testing.T) {
	h := t.TempDir()
	t.Setenv("HOME", h)
	t.Setenv("USERPROFILE", h)
	t.Setenv("XDG_CONFIG_HOME", filepath.Join(h, ".config"))
	t.Setenv("XDG_CACHE_HOME", filepath.Join(h, ".cache"))
	srv := Handler(webHost{func() {}}, nil)
	do := func(method, path, body string) *httptest.ResponseRecorder {
		rec := httptest.NewRecorder()
		srv.ServeHTTP(rec, httptest.NewRequest(method, path, strings.NewReader(body)))
		return rec
	}
	var st privacyModelJSON
	if rec := do("GET", "/api/privacy-model", ""); rec.Code != http.StatusOK || json.Unmarshal(rec.Body.Bytes(), &st) != nil {
		t.Fatalf("state: %d %s", rec.Code, rec.Body)
	}
	if st.On || st.Installed || st.Size == 0 {
		t.Fatalf("state %+v", st)
	}
	if !pii.Supported() {
		if rec := do("POST", "/api/privacy-model", `{"do":"on"}`); rec.Code == http.StatusOK {
			t.Fatalf("on where it can't run: %d %s", rec.Code, rec.Body)
		}
		if settings.Load().RedactModel {
			t.Fatal("saved on with no model")
		}
	}
	if rec := do("POST", "/api/privacy-model", `{"do":"on!"}`); rec.Code == http.StatusOK {
		t.Fatalf("a bad do: %d", rec.Code)
	}
	if rec := do("POST", "/api/settings", `{"theme":"dark","lang":"en","redactModel":true}`); rec.Code != http.StatusOK {
		t.Fatalf("settings save: %d %s", rec.Code, rec.Body)
	}
	if settings.Load().RedactModel {
		t.Fatal("the Settings page's save turned it on")
	}
	if err := settings.SetRedactModel(true); err != nil {
		t.Fatal(err)
	}
	if rec := do("POST", "/api/settings", `{"theme":"dark","lang":"en"}`); rec.Code != http.StatusOK {
		t.Fatalf("settings save: %d %s", rec.Code, rec.Body)
	}
	if !settings.Load().RedactModel {
		t.Fatal("the Settings page's save turned it off")
	}
	if rec := do("POST", "/api/privacy-model", `{"do":"off"}`); rec.Code != http.StatusOK || json.Unmarshal(rec.Body.Bytes(), &st) != nil || st.On {
		t.Fatalf("off: %d %s", rec.Code, rec.Body)
	}
	if settings.Load().RedactModel {
		t.Fatal("off not saved")
	}
}
